//! Media (audio/video) transport for a peer connection.
//!
//! Frames travel on the session's UDP channel, deliberately NOT on the reliable
//! peer channel that carries chat. Two reasons, and the second is the important
//! one:
//!
//!  * A call must not queue behind a file transfer or a burst of messages.
//!  * On a reliable ordered channel there is no such thing as a lost packet —
//!    congestion turns into unbounded latency instead, and a call that is three
//!    seconds behind is worse than one that dropped a frame. Media wants the
//!    lossy channel precisely because loss is the cheaper failure.
//!
//! This module owns framing and transport only. Encoding and decoding happen in
//! the browser via WebCodecs: it is the only path to hardware acceleration, and
//! the WASM build here has neither threads nor SIMD, so in-process codecs could
//! not reach realtime anyway. Payloads are opaque bytes to everything below.
//!
//! The UDP channel is delivered at most ONCE per peer connection, and dropping
//! its receive half tears the UDP path down at the protocol level (its Drop
//! sends DisconnectUDP). [`UdpState`] therefore treats the halves as a durable
//! resource of the peer connection: sessions borrow them and hand them back,
//! so the second and every later call on the same connection can still open.

pub(crate) mod lane;
pub(crate) mod pump;
#[cfg(test)]
mod tests;
mod udp_state;

pub use lane::{media_lane, MediaLaneRx, MediaLaneTx, MEDIA_LANE_CAPACITY};
pub use udp_state::UdpState;

use bytes::BytesMut;
use citadel_internal_service_types::InternalServiceResponse;
use citadel_sdk::citadel_media::{FrameFlags, MediaConfig, Packetizer, TrackId, TrackKind};
use citadel_sdk::logging::info;
use citadel_sdk::prelude::{NetworkError, OutboundUdpSender, PeerChannelRecvHalf, SecBuffer};
use futures::Stream;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use uuid::Uuid;

/// How long to wait for the UDP channel to arrive after a peer connects.
///
/// The channel is negotiated asynchronously after the handshake, so it is
/// normally already here; this only bounds the case where UDP never comes up,
/// so the caller gets a clear failure instead of a call that hangs on "connecting".
pub const UDP_WAIT: Duration = Duration::from_secs(5);

/// Wire tunables. Sized rather than guessed:
///
///  * 1000-byte fragments keep a datagram under a 1500-byte MTU once the wire
///    header and the AEAD overhead of the session's security level are added.
///  * 512 KiB frames comfortably fit a 1080p keyframe; anything larger is a bug
///    in the encoder configuration, and rejecting it before allocation is what
///    stops one bad frame exhausting memory.
///  * A 64-frame reorder window and 60 ms of jitter depth are the usual tradeoff
///    point: enough to absorb ordinary network reordering, short enough that the
///    added latency stays under the ~150 ms where conversation starts to feel
///    like a radio call.
pub(crate) const MEDIA_CONFIG: MediaConfig = MediaConfig {
    max_fragment_payload: 1000,
    max_frame_bytes: 512 * 1024,
    max_reorder_window: 64,
    jitter_depth_micros: 60_000,
    max_pending_frames: 256,
};

/// Largest encoded frame a client may submit; advertised in MediaSessionOpened.
pub const MAX_FRAME_BYTES: u32 = MEDIA_CONFIG.max_frame_bytes as u32;

/// Destination for packetized fragments. Abstracted so the outbound path can be
/// exercised in tests without constructing the SDK's UDP subsystem (SBIO).
pub trait FragmentSink: Send + 'static {
    fn send(&self, fragment: BytesMut) -> Result<(), NetworkError>;
}

impl FragmentSink for OutboundUdpSender {
    fn send(&self, fragment: BytesMut) -> Result<(), NetworkError> {
        self.unbounded_send(fragment)
    }
}

/// The outbound half of a call: packetizer state plus the UDP send half.
///
/// Held behind its own mutex (see [`MediaSession::outbound`]) so that per-frame
/// sends contend only with each other, not with every handler that touches the
/// global connection map.
pub struct MediaOutbound<K: FragmentSink> {
    packetizer: Packetizer,
    sink: K,
}

impl<K: FragmentSink> MediaOutbound<K> {
    /// Built before any transport half is committed, so a config rejection
    /// cannot strand the peer's only UDP channel.
    pub fn new(sink: K) -> Result<Self, NetworkError> {
        let packetizer = Packetizer::new(MEDIA_CONFIG)
            .map_err(|e| NetworkError::msg(format!("invalid media config: {e:?}")))?;
        Ok(Self { packetizer, sink })
    }

    /// Fragment one encoded frame and put it on the wire.
    pub fn send_frame(
        &mut self,
        track: u8,
        kind: u8,
        timestamp: u32,
        flags: u8,
        payload: Vec<u8>,
    ) -> Result<(), NetworkError> {
        let fragments = self
            .packetizer
            .packetize(
                TrackId(track),
                track_kind(kind),
                timestamp,
                // from_bits, not a truncating variant: reserved bits set by a
                // client mean the sender is speaking a dialect we do not know,
                // and silently masking them off would hide that.
                FrameFlags::from_bits(flags)
                    .map_err(|e| NetworkError::msg(format!("invalid frame flags: {e:?}")))?,
                payload.into(),
            )
            .map_err(|e| NetworkError::msg(format!("could not packetize frame: {e:?}")))?;

        for fragment in fragments {
            let mut buf = BytesMut::new();
            fragment.write_into(&mut buf);
            self.sink.send(buf)?;
        }
        Ok(())
    }
}

/// The concrete session type stored on a live peer connection.
pub type PeerMediaSession<R> = MediaSession<PeerChannelRecvHalf<R>, OutboundUdpSender>;

/// A live media session with one peer.
///
/// Boxed where stored because it carries the packetizer's per-track sequence
/// table — over a kilobyte — and peer entries exist whether or not there is
/// ever a call.
pub struct MediaSession<S, K: FragmentSink> {
    outbound: Arc<Mutex<MediaOutbound<K>>>,
    /// The localhost client that opened the call. A MediaOpen from a different
    /// uuid means the browser reconnected; the old session can no longer
    /// deliver inbound frames and must be rebuilt, not confirmed.
    owner: Uuid,
    shutdown: Option<oneshot::Sender<()>>,
    pump: Option<tokio::task::JoinHandle<Option<S>>>,
}

impl<S, K> MediaSession<S, K>
where
    S: Stream<Item = SecBuffer> + Unpin + Send + 'static,
    K: FragmentSink,
{
    /// Start pumping inbound media. Synchronous by design: with the halves in
    /// hand there is nothing to wait for, so callers can run this under the
    /// connection map lock and rule out open/close races entirely.
    pub fn start(
        outbound: MediaOutbound<K>,
        udp_rx: S,
        cid: u64,
        peer_cid: u64,
        owner: Uuid,
        to_client: UnboundedSender<InternalServiceResponse>,
        media_lane: MediaLaneTx,
    ) -> Self {
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let pump = tokio::task::spawn(pump::pump_inbound(
            udp_rx,
            shutdown_rx,
            cid,
            peer_cid,
            to_client,
            media_lane,
        ));
        info!(target: "citadel", "[Media] session open cid={cid} peer_cid={peer_cid}");
        Self {
            outbound: Arc::new(Mutex::new(outbound)),
            owner,
            shutdown: Some(shutdown_tx),
            pump: Some(pump),
        }
    }

    pub fn owner(&self) -> Uuid {
        self.owner
    }

    /// False once the pump has exited — e.g. its client sender died on a
    /// WebSocket reconnect — meaning inbound delivery is permanently broken
    /// even though outbound still works.
    pub fn pump_alive(&self) -> bool {
        self.pump.as_ref().is_some_and(|pump| !pump.is_finished())
    }

    /// Shared handle for per-frame sends, so senders never touch the global map
    /// write lock at frame rate.
    pub fn outbound(&self) -> Arc<Mutex<MediaOutbound<K>>> {
        self.outbound.clone()
    }

    /// Orderly teardown: stop the pump and recover the receive half so the next
    /// call on this peer connection can reuse it. `None` means the UDP stream
    /// itself ended and there is nothing left to lend.
    pub async fn close(mut self) -> Option<S> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let pump = self.pump.take()?;
        pump.await.ok().flatten()
    }
}

/// Last-resort cleanup when a session is dropped without `close()` — i.e. the
/// whole peer entry is going away. Aborting drops the receive half inside the
/// task, which correctly tears down the UDP path along with the connection.
impl<S, K: FragmentSink> Drop for MediaSession<S, K> {
    fn drop(&mut self) {
        if let Some(pump) = self.pump.take() {
            pump.abort();
        }
    }
}

fn track_kind(kind: u8) -> TrackKind {
    // Anything that is not explicitly video is treated as audio, because audio is
    // the stream a call cannot do without — mislabelling video as audio degrades
    // one tile, the reverse would silence the call.
    if kind == TrackKind::Video as u8 {
        TrackKind::Video
    } else {
        TrackKind::Audio
    }
}

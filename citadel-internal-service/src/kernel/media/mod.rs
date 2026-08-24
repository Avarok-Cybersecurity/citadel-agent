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

use citadel_internal_service_types::{
    InternalServiceResponse, MediaFrameNotification, MediaGapNotification,
};
use citadel_sdk::citadel_media::{
    FrameFlags, JitterBuffer, MediaConfig, MediaInstant, Packetizer, PopResult, Reassembler,
    ReassembleOutcome, TrackId, TrackKind,
};
use citadel_sdk::prelude::{NetworkError, OutboundUdpSender, SecBuffer, UdpChannel};
use citadel_sdk::prelude::Ratchet;
use bytes::BytesMut;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot::Receiver as OneshotReceiver;
use citadel_sdk::logging::{info, warn};
use futures::StreamExt;
use std::time::{Duration, Instant};

/// How long to wait for the UDP channel to arrive after a peer connects.
///
/// The channel is negotiated asynchronously after the handshake, so it is
/// normally already here; this only bounds the case where UDP never comes up,
/// so the caller gets a clear failure instead of a call that hangs on "connecting".
const UDP_WAIT: Duration = Duration::from_secs(5);

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
const MEDIA_CONFIG: MediaConfig = MediaConfig {
    max_fragment_payload: 1000,
    max_frame_bytes: 512 * 1024,
    max_reorder_window: 64,
    jitter_depth_micros: 60_000,
    max_pending_frames: 256,
};

/// A live media session with one peer.
pub struct MediaSession {
    packetizer: Packetizer,
    udp_tx: OutboundUdpSender,
    /// Dropping this aborts the inbound pump, which is what releases the UDP
    /// receive half. Without it a closed call would keep decoding frames from a
    /// peer who thinks the call is over.
    _pump: PumpGuard,
}

/// Aborts the inbound pump when the session is dropped.
struct PumpGuard(tokio::task::JoinHandle<()>);

impl Drop for PumpGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl MediaSession {
    pub const fn max_frame_bytes() -> u32 {
        MEDIA_CONFIG.max_frame_bytes as u32
    }

    /// Wait for the peer's UDP channel and start pumping inbound media.
    ///
    /// Returns an error rather than falling back to the reliable channel: a call
    /// silently downgraded to a path that buffers without bound looks like a
    /// working call for about ten seconds and then like a broken product. The
    /// caller surfaces this as a plain "this peer connected without UDP".
    pub async fn open<R: Ratchet>(
        udp_rx: OneshotReceiver<UdpChannel<R>>,
        cid: u64,
        peer_cid: u64,
        to_client: UnboundedSender<InternalServiceResponse>,
    ) -> Result<Self, NetworkError> {
        let channel = tokio::time::timeout(UDP_WAIT, udp_rx)
            .await
            .map_err(|_| {
                NetworkError::msg(format!(
                    "no UDP channel for peer {peer_cid} within {UDP_WAIT:?}; \
                     the peer connection was likely established with UdpMode disabled"
                ))
            })?
            .map_err(|_| NetworkError::msg("UDP channel sender dropped"))?;

        let (udp_tx, udp_rx) = channel.split();
        let packetizer = Packetizer::new(MEDIA_CONFIG)
            .map_err(|e| NetworkError::msg(format!("invalid media config: {e:?}")))?;

        let pump = tokio::task::spawn(pump_inbound(
            udp_rx, cid, peer_cid, to_client,
        ));

        info!(target: "citadel", "[Media] session open cid={cid} peer_cid={peer_cid}");
        Ok(Self {
            packetizer,
            udp_tx,
            _pump: PumpGuard(pump),
        })
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
            self.udp_tx.unbounded_send(buf)?;
        }
        Ok(())
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

/// Reassemble datagrams into frames, release them in order, and report gaps.
async fn pump_inbound<S>(
    mut udp_rx: S,
    cid: u64,
    peer_cid: u64,
    to_client: UnboundedSender<InternalServiceResponse>,
) where
    S: futures::Stream<Item = SecBuffer> + Unpin,
{
    let mut reassembler = match Reassembler::new(MEDIA_CONFIG) {
        Ok(r) => r,
        Err(e) => {
            warn!(target: "citadel", "[Media] reassembler rejected config: {e:?}");
            return;
        }
    };
    let mut jitter = match JitterBuffer::new(MEDIA_CONFIG) {
        Ok(j) => j,
        Err(e) => {
            warn!(target: "citadel", "[Media] jitter buffer rejected config: {e:?}");
            return;
        }
    };
    let origin = Instant::now();

    while let Some(datagram) = udp_rx.next().await {
        let now = MediaInstant::from_micros(origin.elapsed().as_micros() as u64);

        match reassembler.push(datagram.as_ref(), now) {
            ReassembleOutcome::Complete(frame) => {
                let _ = jitter.push(frame, now);
            }
            ReassembleOutcome::Rejected(e) => {
                // Logged once per datagram would be a flood under attack; this is
                // the rare case where losing detail is better than losing the log.
                warn!(target: "citadel", "[Media] rejected datagram from {peer_cid}: {e:?}");
                continue;
            }
            // Partial frames wait for their remaining fragments; duplicates and
            // control messages are not something this transport acts on, since
            // call control travels on the reliable path instead.
            _ => continue,
        }

        loop {
            match jitter.pop_ready(now) {
                PopResult::Frame(frame) => {
                    if !send_frame_to_client(&to_client, cid, peer_cid, &frame) {
                        return; // client gone
                    }
                }
                PopResult::Gap {
                    track,
                    missing_from,
                    missing_to,
                    next,
                } => {
                    // Surfaced, not swallowed: a decoder that resumes after a gap
                    // emits garbage until the next keyframe, so the receiver needs
                    // to know to ask for one.
                    //
                    // The gap arrives WITH the frame that follows it, so both are
                    // forwarded — reporting the gap and dropping `next` would
                    // silently lose a frame every time the network hiccuped.
                    let _ = to_client.send(InternalServiceResponse::MediaGapNotification(
                        MediaGapNotification {
                            cid,
                            peer_cid,
                            track: track.0,
                            missing_from,
                            missing_to,
                            request_id: None,
                        },
                    ));
                    if !send_frame_to_client(&to_client, cid, peer_cid, &next) {
                        return;
                    }
                }
                PopResult::NotReady => break,
            }
        }
    }

    info!(target: "citadel", "[Media] inbound pump ended cid={cid} peer_cid={peer_cid}");
}

/// Returns false once the client is gone, which is the pump's signal to stop.
fn send_frame_to_client(
    to_client: &UnboundedSender<InternalServiceResponse>,
    cid: u64,
    peer_cid: u64,
    frame: &citadel_sdk::citadel_media::MediaFrame,
) -> bool {
    to_client
        .send(InternalServiceResponse::MediaFrameNotification(
            MediaFrameNotification {
                cid,
                peer_cid,
                track: frame.header.track.0,
                kind: frame.header.kind as u8,
                sequence: frame.header.sequence,
                timestamp: frame.header.timestamp,
                flags: frame.header.flags.bits(),
                payload: frame.payload.to_vec(),
                request_id: None,
            },
        ))
        .is_ok()
}

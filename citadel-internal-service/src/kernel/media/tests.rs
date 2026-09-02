//! Unit tests for the media transport that need no live peer: the pump's
//! borrow-and-return contract for the UDP receive half, frame delivery through
//! reassembly + jitter, and the outbound packetization path via a fake sink.

use super::lane::media_lane;
use super::pump::pump_inbound;
use super::{FragmentSink, MediaOutbound, MediaSession, MEDIA_CONFIG};
use bytes::BytesMut;
use citadel_internal_service_types::InternalServiceResponse;
use citadel_sdk::citadel_media::{FrameFlags, Packetizer, TrackId, TrackKind};
use citadel_sdk::prelude::{NetworkError, SecBuffer};
use futures::channel::mpsc as futures_mpsc;
use std::time::Duration;
use uuid::Uuid;

type ClientRx = tokio::sync::mpsc::UnboundedReceiver<InternalServiceResponse>;
type ClientTx = tokio::sync::mpsc::UnboundedSender<InternalServiceResponse>;

struct FakeSink(tokio::sync::mpsc::UnboundedSender<BytesMut>);

impl FragmentSink for FakeSink {
    fn send(&self, fragment: BytesMut) -> Result<(), NetworkError> {
        self.0
            .send(fragment)
            .map_err(|e| NetworkError::msg(e.to_string()))
    }
}

fn client_channel() -> (ClientTx, ClientRx) {
    tokio::sync::mpsc::unbounded_channel()
}

/// Packetizes one frame and renders its fragments as on-the-wire datagrams.
fn frame_datagrams(
    packetizer: &mut Packetizer,
    timestamp: u32,
    payload: Vec<u8>,
) -> Vec<SecBuffer> {
    packetizer
        .packetize(
            TrackId(0),
            TrackKind::Audio,
            timestamp,
            FrameFlags::NONE,
            payload.into(),
        )
        .expect("packetize")
        .map(|fragment| {
            let mut buf = BytesMut::new();
            fragment.write_into(&mut buf);
            SecBuffer::from(buf.to_vec())
        })
        .collect()
}

/// Feeds two frames separated by more than the jitter hold-back so the buffer
/// locks on and releases the first frame.
async fn feed_two_frames(datagram_tx: &futures_mpsc::UnboundedSender<SecBuffer>) {
    let mut packetizer = Packetizer::new(MEDIA_CONFIG).expect("packetizer");
    for datagram in frame_datagrams(&mut packetizer, 0, vec![7u8; 2500]) {
        datagram_tx.unbounded_send(datagram).expect("send datagram");
    }
    // Real-time sleep: the jitter buffer holds the first frame back for
    // jitter_depth_micros (60 ms) measured against a wall-clock origin.
    tokio::time::sleep(Duration::from_millis(100)).await;
    for datagram in frame_datagrams(&mut packetizer, 1, vec![9u8; 100]) {
        datagram_tx.unbounded_send(datagram).expect("send datagram");
    }
}

#[tokio::test]
async fn pump_returns_receive_half_on_shutdown() {
    let (_datagram_tx, datagram_rx) = futures_mpsc::unbounded::<SecBuffer>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let (client_tx, _client_rx) = client_channel();
    let (lane_tx, _lane_rx) = media_lane(8);

    let pump = tokio::spawn(pump_inbound(
        datagram_rx,
        shutdown_rx,
        1,
        2,
        client_tx,
        lane_tx,
    ));
    shutdown_tx.send(()).expect("pump alive");
    let recovered = pump.await.expect("pump join");
    assert!(recovered.is_some(), "orderly close must return the rx half");
}

#[tokio::test]
async fn pump_reports_dead_path_when_stream_ends() {
    let (datagram_tx, datagram_rx) = futures_mpsc::unbounded::<SecBuffer>();
    let (_shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let (client_tx, _client_rx) = client_channel();
    let (lane_tx, _lane_rx) = media_lane(8);

    let pump = tokio::spawn(pump_inbound(
        datagram_rx,
        shutdown_rx,
        1,
        2,
        client_tx,
        lane_tx,
    ));
    drop(datagram_tx);
    let recovered = pump.await.expect("pump join");
    assert!(recovered.is_none(), "an exhausted rx must not be re-parked");
}

#[tokio::test]
async fn pump_delivers_reassembled_frames_in_order() {
    let (datagram_tx, datagram_rx) = futures_mpsc::unbounded::<SecBuffer>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let (client_tx, _client_rx) = client_channel();
    // Read from the media lane, not the control channel: frames were moved onto
    // their own bounded queue, and asserting on the old one would pass only for
    // as long as nothing was actually separated.
    let (lane_tx, mut lane_rx) = media_lane(8);

    let pump = tokio::spawn(pump_inbound(
        datagram_rx,
        shutdown_rx,
        11,
        22,
        client_tx,
        lane_tx,
    ));
    feed_two_frames(&datagram_tx).await;

    let notification = tokio::time::timeout(Duration::from_secs(5), lane_rx.recv())
        .await
        .expect("frame within deadline")
        .expect("media lane open");
    match notification {
        InternalServiceResponse::MediaFrameNotification(frame) => {
            assert_eq!(frame.cid, 11);
            assert_eq!(frame.peer_cid, 22);
            assert_eq!(frame.track, 0);
            assert_eq!(frame.payload, vec![7u8; 2500]);
        }
        other => panic!("expected MediaFrameNotification, got {other:?}"),
    }

    shutdown_tx.send(()).expect("pump alive");
    assert!(pump.await.expect("pump join").is_some());
}

#[tokio::test]
async fn pump_survives_client_loss_and_returns_receive_half() {
    let (datagram_tx, datagram_rx) = futures_mpsc::unbounded::<SecBuffer>();
    let (_shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let (client_tx, client_rx) = client_channel();
    drop(client_rx); // the browser's WebSocket dropped
                     // The lane is closed on the same event, and it is the lane the frame path
                     // now consults to discover the client is gone.
    let (lane_tx, _lane_rx) = media_lane(8);
    lane_tx.close();

    let pump = tokio::spawn(pump_inbound(
        datagram_rx,
        shutdown_rx,
        1,
        2,
        client_tx,
        lane_tx,
    ));
    // Delivery of the first frame fails against the dead client, which must end
    // the pump WITHOUT consuming the receive half.
    feed_two_frames(&datagram_tx).await;
    let recovered = tokio::time::timeout(Duration::from_secs(5), pump)
        .await
        .expect("pump ends after client loss")
        .expect("pump join");
    assert!(recovered.is_some(), "rx must survive a client disconnect");
}

#[tokio::test]
async fn session_close_recovers_receive_half_and_reports_dead_pump() {
    let (fragment_tx, _fragment_rx) = tokio::sync::mpsc::unbounded_channel();
    let (_datagram_tx, datagram_rx) = futures_mpsc::unbounded::<SecBuffer>();
    let (client_tx, _client_rx) = client_channel();
    let owner = Uuid::new_v4();

    let outbound = MediaOutbound::new(FakeSink(fragment_tx)).expect("outbound");
    let session = MediaSession::start(
        outbound,
        datagram_rx,
        1,
        2,
        owner,
        client_tx,
        media_lane(8).0,
    );
    assert_eq!(session.owner(), owner);
    assert!(session.pump_alive());
    let recovered = session.close().await;
    assert!(recovered.is_some(), "close must hand the rx half back");
}

#[tokio::test]
async fn outbound_fragments_frames_and_rejects_unknown_flags() {
    let (fragment_tx, mut fragment_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut outbound = MediaOutbound::new(FakeSink(fragment_tx)).expect("outbound");

    // 2500 bytes at 1000-byte fragments must yield three datagrams.
    outbound
        .send_frame(0, TrackKind::Audio as u8, 0, 0, vec![1u8; 2500])
        .expect("send_frame");
    let mut fragments = 0;
    while fragment_rx.try_recv().is_ok() {
        fragments += 1;
    }
    assert_eq!(fragments, 3);

    // Reserved flag bits are a protocol dialect we do not speak: reject loudly.
    assert!(outbound
        .send_frame(0, TrackKind::Audio as u8, 0, 0b1000_0000, vec![1u8; 10])
        .is_err());
}

mod no_media_reporting {
    use super::super::pump::should_report_no_media;

    /// The case nobody could diagnose: frames arriving, every one dropped,
    /// nothing delivered, and the pump silent about all of it.
    #[test]
    fn a_call_that_delivers_nothing_is_reported() {
        assert!(should_report_no_media(false, 0, 30));
        assert!(should_report_no_media(false, 0, 5_000));
    }

    /// Loss alongside delivery is ordinary for real-time media. Reporting it
    /// would train the reader to ignore the message that matters.
    #[test]
    fn a_lossy_call_that_is_working_stays_quiet() {
        assert!(!should_report_no_media(false, 1, 5_000));
        assert!(!should_report_no_media(false, 10_000, 10_000));
    }

    /// Said once. A pump dropping every frame drops thousands of them, and the
    /// log has to stay readable.
    #[test]
    fn it_is_said_only_once() {
        assert!(!should_report_no_media(true, 0, 5_000));
    }

    /// A handful of drops at startup is reordering settling, not a dead call.
    #[test]
    fn a_brief_startup_reorder_does_not_trip_it() {
        assert!(!should_report_no_media(false, 0, 1));
        assert!(!should_report_no_media(false, 0, 29));
    }
}

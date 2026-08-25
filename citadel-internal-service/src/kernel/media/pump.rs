//! Inbound pump: reassemble datagrams into frames, release them in order, and
//! report gaps.
//!
//! The pump BORROWS the UDP receive half and returns it when it stops, because
//! the half is issued once per peer connection and its Drop tears the UDP path
//! down at the protocol level. `Some(rx)` on exit means the half is reusable by
//! a later call; `None` means the stream itself ended and the path is gone.

use super::lane::{MediaLaneTx, PushOutcome};
use super::MEDIA_CONFIG;
use citadel_internal_service_types::{
    InternalServiceResponse, MediaFrameNotification, MediaGapNotification,
};
use citadel_sdk::citadel_media::{
    JitterBuffer, MediaInstant, PopResult, ReassembleOutcome, Reassembler,
};
use citadel_sdk::logging::{debug, info, warn};
use citadel_sdk::prelude::SecBuffer;
use futures::StreamExt;
use std::time::Instant;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot::Receiver as ShutdownReceiver;

pub(crate) async fn pump_inbound<S>(
    mut udp_rx: S,
    mut shutdown: ShutdownReceiver<()>,
    cid: u64,
    peer_cid: u64,
    to_client: UnboundedSender<InternalServiceResponse>,
    media_lane: MediaLaneTx,
) -> Option<S>
where
    S: futures::Stream<Item = SecBuffer> + Unpin,
{
    let mut reassembler = match Reassembler::new(MEDIA_CONFIG) {
        Ok(reassembler) => reassembler,
        Err(e) => {
            warn!(target: "citadel", "[Media] reassembler rejected config: {e:?}");
            return Some(udp_rx);
        }
    };
    let mut jitter = match JitterBuffer::new(MEDIA_CONFIG) {
        Ok(jitter) => jitter,
        Err(e) => {
            warn!(target: "citadel", "[Media] jitter buffer rejected config: {e:?}");
            return Some(udp_rx);
        }
    };
    let origin = Instant::now();

    'pump: loop {
        let datagram = tokio::select! {
            biased;
            // Orderly close beats data: the session owner asked us to stop, and
            // draining first could hold the receive half hostage under load.
            _ = &mut shutdown => break 'pump,
            datagram = udp_rx.next() => match datagram {
                Some(datagram) => datagram,
                None => {
                    info!(target: "citadel", "[Media] UDP stream ended cid={cid} peer_cid={peer_cid}");
                    return None;
                }
            },
        };
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
                    if !send_frame_to_client(&media_lane, cid, peer_cid, &frame) {
                        // Client gone (typically a WebSocket reconnect). The
                        // receive half is still healthy — hand it back so the
                        // reconnected client's re-open can rebuild the session.
                        break 'pump;
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
                    if !send_frame_to_client(&media_lane, cid, peer_cid, &next) {
                        break 'pump;
                    }
                }
                PopResult::NotReady => break,
            }
        }
    }

    info!(target: "citadel", "[Media] inbound pump ended cid={cid} peer_cid={peer_cid}");
    Some(udp_rx)
}

/// Returns false once the client is gone, which is the pump's signal to stop.
///
/// Frames go on the bounded lane, gaps on the reliable one. A dropped frame
/// costs a sixtieth of a second; a dropped gap leaves the receiver's decoder
/// emitting garbage because it never learned to ask for a keyframe. They travel
/// the same socket and must not share a drop policy.
fn send_frame_to_client(
    media_lane: &MediaLaneTx,
    cid: u64,
    peer_cid: u64,
    frame: &citadel_sdk::citadel_media::MediaFrame,
) -> bool {
    let outcome = media_lane.push(InternalServiceResponse::MediaFrameNotification(
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
    ));

    match outcome {
        PushOutcome::Closed => false,
        // Logged once per eviction at debug: this is expected behaviour under
        // congestion, not a fault, and the running total is on the lane for
        // anyone who wants the rate rather than the events.
        PushOutcome::DroppedOldest => {
            debug!(
                target: "citadel",
                "[Media] client behind, evicting oldest frame cid={cid} peer_cid={peer_cid} dropped_total={}",
                media_lane.dropped()
            );
            true
        }
        PushOutcome::Queued => true,
    }
}

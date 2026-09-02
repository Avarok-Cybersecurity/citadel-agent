//! Commit phase of MediaOpen: the code that runs after an unavoidable await
//! (waiting for the UDP channel, or shutting down a stale session) and must
//! therefore re-validate the peer's `media_generation` before installing
//! anything. Split from `open.rs` piecewise to respect the file-size limit.

use super::{failed, opened, park_recovered_receive_half};
use crate::kernel::media::{MediaLaneTx, MediaOutbound, PeerMediaSession, UdpState, UDP_WAIT};
use crate::kernel::{CitadelWorkspaceService, PeerConnection};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::InternalServiceResponse;
use citadel_sdk::prelude::{OutboundUdpSender, PeerChannelRecvHalf, Ratchet, UdpChannel};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot::error::RecvError;
use tokio::sync::oneshot::Receiver as OneshotReceiver;
use tokio::time::error::Elapsed;
use uuid::Uuid;

pub(super) type ClientSender = UnboundedSender<InternalServiceResponse>;
pub(super) type ChannelOutcome<R> = Result<Result<UdpChannel<R>, RecvError>, Elapsed>;

/// Commits a session from halves already in hand. Runs under the map lock —
/// spawning the pump and building the packetizer are both synchronous.
#[allow(clippy::too_many_arguments)]
pub(super) fn start_session<R: Ratchet>(
    peer: &mut PeerConnection<R>,
    tx: OutboundUdpSender,
    rx: PeerChannelRecvHalf<R>,
    cid: u64,
    peer_cid: u64,
    owner: Uuid,
    request_id: Uuid,
    to_client: ClientSender,
    media_lane: MediaLaneTx,
) -> InternalServiceResponse {
    match MediaOutbound::new(tx.clone()) {
        Ok(outbound) => {
            let session =
                PeerMediaSession::start(outbound, rx, cid, peer_cid, owner, to_client, media_lane);
            peer.udp = UdpState::Lent { tx };
            peer.media = Some(Box::new(session));
            opened(cid, peer_cid, request_id)
        }
        // Config rejection must not strand the connection's only UDP channel:
        // park the halves so the failure is retryable.
        Err(e) => {
            peer.udp = UdpState::Idle { tx, rx };
            failed(cid, peer_cid, request_id, e.to_string())
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn finish_first_open<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    peer_cid: u64,
    uuid: Uuid,
    request_id: Uuid,
    to_client: ClientSender,
    media_lane: MediaLaneTx,
    rxs: Vec<OneshotReceiver<UdpChannel<R>>>,
    outcome: ChannelOutcome<R>,
    generation: u64,
) -> InternalServiceResponse {
    let mut map = this.server_connection_map.write();
    let Some(peer) = map
        .get_mut(&cid)
        .and_then(|conn| conn.peers.get_mut(&peer_cid))
    else {
        // Whatever arrived is dropped along with the peer, which is the correct
        // teardown for a connection that no longer exists.
        return failed(
            cid,
            peer_cid,
            request_id,
            "peer disconnected while the media session was opening".to_string(),
        );
    };

    // The await is over either way, so the claim is released here rather than on
    // the success path alone: leaving it set after a failed open would let the
    // dead connection keep authority over the next open's cancellation.
    if peer.media_pending_owner == Some(uuid) {
        peer.media_pending_owner = None;
    }

    // Ours only if no close bumped the generation AND no re-handshake replaced
    // the transport out from under our `Opening` marker.
    if peer.media_generation != generation || !matches!(peer.udp, UdpState::Opening) {
        if matches!(peer.udp, UdpState::Opening) {
            // A close raced this open. The client believes the call is over, so
            // no session may be installed — but the channel must be preserved
            // for the NEXT call, whichever way the wait ended.
            peer.udp = match outcome {
                Ok(Ok(channel)) => {
                    let (tx, rx) = channel.split();
                    UdpState::Idle { tx, rx }
                }
                Ok(Err(_)) => UdpState::Unavailable,
                Err(_) => UdpState::Pending(rxs),
            };
        }
        return failed(
            cid,
            peer_cid,
            request_id,
            "the media session was closed while it was opening".to_string(),
        );
    }

    match outcome {
        Ok(Ok(channel)) => {
            let (tx, rx) = channel.split();
            start_session(
                peer, tx, rx, cid, peer_cid, uuid, request_id, to_client, media_lane,
            )
        }
        Ok(Err(_)) => {
            peer.udp = UdpState::Unavailable;
            failed(
                cid,
                peer_cid,
                request_id,
                "the peer connection withdrew its UDP channel offer; reconnect to the peer \
                 with UDP enabled to place a call"
                    .to_string(),
            )
        }
        Err(_) => {
            // The receiver survives the timeout (awaited via &mut), so this is
            // retryable: the channel may simply still be negotiating.
            peer.udp = UdpState::Pending(rxs);
            failed(
                cid,
                peer_cid,
                request_id,
                format!(
                    "no UDP channel for peer {peer_cid} within {UDP_WAIT:?}; it may still be \
                     negotiating (retry shortly), or the peer connection was established with \
                     UdpMode disabled"
                ),
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn rebuild_after_stale<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    peer_cid: u64,
    uuid: Uuid,
    request_id: Uuid,
    to_client: ClientSender,
    media_lane: MediaLaneTx,
    recovered: Option<PeerChannelRecvHalf<R>>,
    generation: u64,
) -> InternalServiceResponse {
    {
        let mut map = this.server_connection_map.write();
        let Some(peer) = map
            .get_mut(&cid)
            .and_then(|conn| conn.peers.get_mut(&peer_cid))
        else {
            return failed(
                cid,
                peer_cid,
                request_id,
                "peer disconnected while the media session was rebuilding".to_string(),
            );
        };

        if super::open_may_commit(generation, peer.media_generation, peer.media.is_some()) {
            return match (
                recovered,
                std::mem::replace(&mut peer.udp, UdpState::Opening),
            ) {
                (Some(rx), UdpState::Lent { tx }) => start_session(
                    peer, tx, rx, cid, peer_cid, uuid, request_id, to_client, media_lane,
                ),
                (None, UdpState::Lent { .. }) => {
                    peer.udp = UdpState::Unavailable;
                    failed(
                        cid,
                        peer_cid,
                        request_id,
                        "the UDP path to this peer has ended; reconnect to the peer to place \
                         a call"
                            .to_string(),
                    )
                }
                // A fresh handshake replaced the transport mid-rebuild; the
                // recovered half belongs to the old path and is dropped.
                (_stale, state) => {
                    peer.udp = state;
                    failed(
                        cid,
                        peer_cid,
                        request_id,
                        "the peer transport was replaced while the media session was \
                         rebuilding; retry"
                            .to_string(),
                    )
                }
            };
        }
        // A close (or another open) intervened while the stale session was shutting
        // down; fall through to park the transport outside this lock scope.
    }
    park_recovered_receive_half(this, cid, peer_cid, recovered);
    failed(
        cid,
        peer_cid,
        request_id,
        "the media session was closed while it was being rebuilt; retry".to_string(),
    )
}

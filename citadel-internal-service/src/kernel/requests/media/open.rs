//! MediaOpen: the only media handler that can wait, and therefore the only one
//! that must survive races with MediaClose, with concurrent opens, and with the
//! peer re-handshaking. The discipline: decide everything possible under one
//! lock; when a wait is unavoidable, capture `media_generation` first and
//! refuse to commit if it moved.

use super::commit::{finish_first_open, rebuild_after_stale, start_session};
use super::{failed, opened};
use crate::kernel::media::{PeerMediaSession, UdpState, UDP_WAIT};
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
use citadel_sdk::logging::info;
use citadel_sdk::prelude::{Ratchet, UdpChannel};
use tokio::sync::oneshot::Receiver as OneshotReceiver;
use uuid::Uuid;

/// What the first locked inspection decided. Only the paths that must await
/// leave the lock; both re-validate the generation before committing.
enum OpenPath<R: Ratchet> {
    // Boxed: a response can be hundreds of bytes and would otherwise size the
    // whole enum (clippy::large_enum_variant).
    Done(Box<InternalServiceResponse>),
    AwaitChannel {
        rx: OneshotReceiver<UdpChannel<R>>,
        generation: u64,
    },
    RebuildStale {
        session: Box<PeerMediaSession<R>>,
        generation: u64,
    },
}

pub async fn handle_open<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::MediaOpen {
        request_id,
        cid,
        peer_cid,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };

    let to_client = this.tx_to_localhost_clients.read().get(&uuid).cloned();
    let Some(to_client) = to_client else {
        return Some(HandledRequestResult {
            response: failed(cid, peer_cid, request_id, "client is gone".to_string()),
            uuid,
        });
    };

    let path = {
        let mut map = this.server_connection_map.write();
        let Some(peer) = map
            .get_mut(&cid)
            .and_then(|conn| conn.peers.get_mut(&peer_cid))
        else {
            return Some(HandledRequestResult {
                response: failed(
                    cid,
                    peer_cid,
                    request_id,
                    format!("no peer connection to {peer_cid}; connect before starting a call"),
                ),
                uuid,
            });
        };

        if let Some(session) = peer.media.as_ref() {
            // Idempotent only when the existing session can still deliver. After
            // a WebSocket reconnect the pump has exited (its client sender died)
            // and/or the requester holds a fresh uuid; confirming such a session
            // would produce a one-way call, so it is torn down and rebuilt.
            if session.pump_alive() && session.owner() == uuid {
                info!(target: "citadel", "[Media] session already open cid={cid} peer_cid={peer_cid}");
                OpenPath::Done(Box::new(opened(cid, peer_cid, request_id)))
            } else {
                info!(target: "citadel", "[Media] rebuilding stale session cid={cid} peer_cid={peer_cid}");
                peer.media_generation += 1;
                let generation = peer.media_generation;
                let session = peer.media.take().expect("checked Some above");
                OpenPath::RebuildStale {
                    session,
                    generation,
                }
            }
        } else {
            match std::mem::replace(&mut peer.udp, UdpState::Opening) {
                UdpState::Pending(rx) => {
                    // Claimed before the await so a close arriving from a stale
                    // connection can tell whose open it would be cancelling.
                    peer.media_pending_owner = Some(uuid);
                    OpenPath::AwaitChannel {
                        rx,
                        generation: peer.media_generation,
                    }
                }
                // Reopen on parked halves: nothing to wait for, so the whole
                // open commits under this one lock and cannot race a close.
                UdpState::Idle { tx, rx } => OpenPath::Done(Box::new(start_session(
                    peer,
                    tx,
                    rx,
                    cid,
                    peer_cid,
                    uuid,
                    request_id,
                    to_client.clone(),
                ))),
                // Distinguished from "no UDP path": the channel exists, it is
                // just held by a concurrent open or a close still in flight.
                state @ (UdpState::Opening | UdpState::Lent { .. }) => {
                    peer.udp = state;
                    OpenPath::Done(Box::new(failed(
                        cid,
                        peer_cid,
                        request_id,
                        "a media open or teardown is already in progress with this peer; \
                         retry shortly"
                            .to_string(),
                    )))
                }
                UdpState::Unavailable => {
                    peer.udp = UdpState::Unavailable;
                    OpenPath::Done(Box::new(failed(
                        cid,
                        peer_cid,
                        request_id,
                        "this peer connection has no usable UDP path, either because it was \
                         established with UdpMode disabled or because the UDP channel ended; \
                         reconnect to the peer to place a call"
                            .to_string(),
                    )))
                }
            }
        }
    };

    let response = match path {
        OpenPath::Done(response) => *response,
        OpenPath::AwaitChannel { mut rx, generation } => {
            // Awaiting through &mut keeps the receiver alive on timeout so a
            // later open can retry; consuming it here is what used to kill media
            // on this peer connection forever after one failed open.
            let outcome = tokio::time::timeout(UDP_WAIT, &mut rx).await;
            finish_first_open(
                this, cid, peer_cid, uuid, request_id, to_client, rx, outcome, generation,
            )
        }
        OpenPath::RebuildStale {
            session,
            generation,
        } => {
            let recovered = session.close().await;
            rebuild_after_stale(
                this, cid, peer_cid, uuid, request_id, to_client, recovered, generation,
            )
        }
    };

    Some(HandledRequestResult { response, uuid })
}

use crate::kernel::requests::HandledRequestResult;
use crate::kernel::{AsyncSink, CitadelWorkspaceService};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, MessageSendFailure, MessageSendSuccess,
};
use citadel_sdk::logging::info;
use citadel_sdk::prelude::{Ratchet, SecurityLevel};
use std::time::Duration;
use uuid::Uuid;

/// How long one message may spend waiting for a peer's sink, and then on it.
///
/// Every request runs in its own spawned task, and this sink is behind a mutex
/// shared by every message to that peer. With no bound, a wedged peer collects
/// tasks: each one parks on `lock().await` or inside `send().await` forever, the
/// caller is never answered, and nothing caps how many accumulate. Memory grows
/// for as long as the application keeps sending, and the UI shows messages stuck
/// on 'sent' with no failure to act on.
///
/// Thirty seconds is deliberately generous — the same figure the peer-list and
/// deregister paths use — because the cost of being wrong in the impatient
/// direction is a spurious failure on a slow but working link. The point is not
/// promptness, it is that the wait ENDS.
const PEER_SEND_TIMEOUT: Duration = Duration::from_secs(30);

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::Message {
        request_id,
        message,
        cid,
        peer_cid,
        security_level,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };

    // Clone the sink Arc BEFORE dropping the lock - this is the async-safe pattern
    // that avoids holding the RwLock across await points
    let sink_result: Result<(AsyncSink<R>, SecurityLevel), String> = {
        let server_connection_map = this.server_connection_map.read();
        match server_connection_map.get(&cid) {
            Some(conn) => {
                if let Some(peer_cid) = peer_cid {
                    // send to peer
                    info!(target: "citadel", "[P2P-MSG] Sending message from {cid} to peer {peer_cid}");
                    info!(target: "citadel", "[P2P-MSG] Available peers in conn.peers: {:?}", conn.peers.keys().collect::<Vec<_>>());
                    if let Some(peer_conn) = conn.peers.get(&peer_cid) {
                        info!(target: "citadel", "[P2P-MSG] Found peer connection, cloning sink Arc");
                        Ok((peer_conn.sink.clone(), security_level))
                    } else {
                        citadel_sdk::logging::error!(target: "citadel","[P2P-MSG] Peer connection not found for peer_cid={peer_cid}");
                        Err(format!("Peer connection for {peer_cid} not found"))
                    }
                } else {
                    // send to server
                    info!(target: "citadel", "[P2P-MSG] Sending message from {cid} to SERVER (no peer_cid)");
                    Ok((conn.sink_to_server.clone(), security_level))
                }
            }
            None => {
                info!(target: "citadel", "connection not found");
                Err(format!("Connection for {cid} not found"))
            }
        }
    }; // RwLock dropped here - BEFORE any await

    match sink_result {
        Ok((sink, security_level)) => {
            // Bounded, both halves. The lock is shared by every message to this
            // peer and the send can block on a wedged link; see PEER_SEND_TIMEOUT
            // for why an unbounded wait accumulates tasks instead of failing.
            let send_result = tokio::time::timeout(PEER_SEND_TIMEOUT, async {
                let mut sink_guard = sink.lock().await;
                sink_guard.set_security_level(security_level);
                info!(target: "citadel", "[P2P-MSG] About to call sink.send() for message from {} to {:?}", cid, peer_cid);
                sink_guard.send(message).await
            })
            .await;

            let send_result = match send_result {
                Ok(result) => result,
                Err(_elapsed) => {
                    citadel_sdk::logging::warn!(target: "citadel", "[P2P-MSG] Timed out after {PEER_SEND_TIMEOUT:?} sending from {cid} to {peer_cid:?}; the peer's sink is not draining");
                    let response =
                        InternalServiceResponse::MessageSendFailure(MessageSendFailure {
                            cid,
                            message: format!(
                                "Timed out after {PEER_SEND_TIMEOUT:?} waiting to send to {peer_cid:?}"
                            ),
                            request_id: Some(request_id),
                        });
                    return Some(HandledRequestResult { response, uuid });
                }
            };

            if let Err(err) = send_result {
                let response = InternalServiceResponse::MessageSendFailure(MessageSendFailure {
                    cid,
                    message: format!("Error sending message: {err:?}"),
                    request_id: Some(request_id),
                });
                Some(HandledRequestResult { response, uuid })
            } else {
                info!(target: "citadel", "[P2P-MSG] sink.send() SUCCEEDED for message from {} to {:?}", cid, peer_cid);
                let response = InternalServiceResponse::MessageSendSuccess(MessageSendSuccess {
                    cid,
                    peer_cid,
                    request_id: Some(request_id),
                });
                Some(HandledRequestResult { response, uuid })
            }
        }
        Err(error_msg) => {
            let response = InternalServiceResponse::MessageSendFailure(MessageSendFailure {
                cid,
                message: error_msg,
                request_id: Some(request_id),
            });
            Some(HandledRequestResult { response, uuid })
        }
    }
}

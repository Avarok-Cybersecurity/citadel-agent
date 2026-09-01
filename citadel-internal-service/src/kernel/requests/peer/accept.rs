use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, PeerConnectAcceptFailure,
    PeerConnectAcceptSuccess,
};
use citadel_sdk::logging::{error, info};
use citadel_sdk::prelude::Ratchet;
use citadel_sdk::responses;
use uuid::Uuid;

/// Handle PeerConnectAccept request - respond to an incoming P2P connection request.
///
/// When a peer initiates a connection via PeerConnect, the internal service receives
/// a PeerSignal::PostConnect which is stored in `pending_peer_connect_signals` and
/// forwarded to the UI as PeerConnectNotification. The UI then sends PeerConnectAccept
/// to accept (or decline) the connection.
///
/// Flow:
/// 1. Peer A calls PeerConnect → sends PostConnect to server
/// 2. Server routes PostConnect to Peer B
/// 3. Internal service stores signal, sends PeerConnectNotification to UI
/// 4. UI sends PeerConnectAccept back
/// 5. This handler retrieves stored signal, calls responses::peer_connect
/// 6. SDK completes the connection handshake
pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::PeerConnectAccept {
        request_id,
        cid,
        peer_cid,
        accept,
        udp_mode: _,
        session_security_settings: _,
        peer_session_password,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };

    info!(target: "citadel", "[PeerConnectAccept] Received request: cid={}, peer_cid={}, accept={}", cid, peer_cid, accept);

    // IDEMPOTENCY CHECK: If peer is already connected, return success immediately.
    // This prevents duplicate PeerConnectAccept requests (from race conditions in
    // multi-tab scenarios) from failing after the first one succeeds.
    {
        let conns = this.server_connection_map.read();
        if let Some(conn) = conns.get(&cid) {
            if conn.peers.contains_key(&peer_cid) {
                info!(target: "citadel", "[PeerConnectAccept] Peer {} already connected to {} - idempotent success", peer_cid, cid);
                return Some(HandledRequestResult {
                    response: InternalServiceResponse::PeerConnectAcceptSuccess(
                        PeerConnectAcceptSuccess {
                            cid,
                            peer_cid,
                            request_id: Some(request_id),
                        },
                    ),
                    uuid,
                });
            }
        }
    }

    // Log current pending signals for debugging
    {
        let pending = this.pending_peer_connect_signals.read();
        info!(target: "citadel", "[PeerConnectAccept] Current pending signals count: {}", pending.len());
        for key in pending.keys() {
            info!(target: "citadel", "[PeerConnectAccept]   - Pending signal key: (cid={}, peer_cid={})", key.0, key.1);
        }
    }

    // Retrieve the stored pending signal
    let pending_signal = this
        .pending_peer_connect_signals
        .write()
        .remove(&(cid, peer_cid));

    let Some(signal) = pending_signal else {
        error!(target: "citadel", "[PeerConnectAccept] No pending signal found for ({}, {})", cid, peer_cid);
        return Some(HandledRequestResult {
            response: InternalServiceResponse::PeerConnectAcceptFailure(PeerConnectAcceptFailure {
                cid,
                peer_cid,
                message: format!("No pending connection request from peer {}", peer_cid),
                request_id: Some(request_id),
            }),
            uuid,
        });
    };

    info!(target: "citadel", "[PeerConnectAccept] Found pending signal, calling peer_connect response");

    // Get the remote to send the response
    let remote = this.remote();

    // Call the SDK's peer_connect response function
    // NOTE for whoever adds a decline path to the UI.
    //
    // Both outcomes answer with PeerConnectAcceptSuccess. It is accurate from
    // here — the response WAS delivered — but it means the receiver cannot tell
    // "they accepted" from "your refusal was sent", and the `accept` flag this
    // function branched on is not carried back.
    //
    // PeerRegisterRespond had exactly this shape and it was a live defect:
    // declining a registration ran the frontend's acceptance path, marked the
    // declined peer registered, and had p2p-auto-connect open a connection to
    // the person just refused. See citadel-workspaces'
    // p2p-registration-service/decline-correlation.ts.
    //
    // This one is not reachable today only because nothing sends accept:false —
    // incoming connections are auto-accepted, consent having been given at
    // registration. Adding a decline without carrying the outcome back would
    // reintroduce that bug. FileTransferStatusNotification shows the shape to
    // copy: it carries `success` AND `response`.
    match responses::peer_connect(signal, accept, remote, peer_session_password).await {
        Ok(ticket) => {
            info!(target: "citadel", "[PeerConnectAccept] Successfully sent {} response, ticket={:?}",
                if accept { "accept" } else { "decline" }, ticket);
            Some(HandledRequestResult {
                response: InternalServiceResponse::PeerConnectAcceptSuccess(
                    PeerConnectAcceptSuccess {
                        cid,
                        peer_cid,
                        request_id: Some(request_id),
                    },
                ),
                uuid,
            })
        }
        Err(err) => {
            let err_str = err.into_string();
            error!(target: "citadel", "[PeerConnectAccept] Failed to send response: {}", err_str);
            Some(HandledRequestResult {
                response: InternalServiceResponse::PeerConnectAcceptFailure(
                    PeerConnectAcceptFailure {
                        cid,
                        peer_cid,
                        message: err_str,
                        request_id: Some(request_id),
                    },
                ),
                uuid,
            })
        }
    }
}

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

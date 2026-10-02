use crate::kernel::requests::peer::answer::{answer_offer, remember_window_relay};
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, PeerConnectAcceptFailure,
    PeerConnectAcceptSuccess,
};
use citadel_sdk::logging::{error, info};
use citadel_sdk::prelude::Ratchet;
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
///
/// For an account the agent hosts, step 4 is the agent's own
/// (kernel/inbound_connect): the account is reachable with no window open, and
/// the notification says so (`answered_by_agent`) so no window answers too.
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
        turn,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };

    info!(target: "citadel", "[PeerConnectAccept] Received request: cid={}, peer_cid={}, accept={}", cid, peer_cid, accept);
    if accept {
        remember_window_relay(this, cid, turn.as_ref());
    }

    // Both outcomes answer with PeerConnectAcceptSuccess, because both ARE
    // successes: the answer was delivered. What the response carries is WHICH
    // answer, in `accept`.
    //
    // Without that field the type name was the entire message and it said
    // "success" either way, so a receiver could not tell "they accepted" from
    // "your refusal was sent". PeerRegisterRespond had exactly that shape and
    // it was live: declining a registration ran the frontend's acceptance path,
    // marked the declined peer registered, and had auto-connect open a
    // connection to the person just refused.
    let response = match answer_offer(
        this,
        cid,
        peer_cid,
        accept,
        turn.as_ref(),
        peer_session_password,
    )
    .await
    {
        Ok(()) => InternalServiceResponse::PeerConnectAcceptSuccess(PeerConnectAcceptSuccess {
            cid,
            peer_cid,
            accept,
            request_id: Some(request_id),
        }),
        Err(message) => {
            error!(target: "citadel", "[PeerConnectAccept] ({cid}, {peer_cid}) failed: {message}");
            InternalServiceResponse::PeerConnectAcceptFailure(PeerConnectAcceptFailure {
                cid,
                peer_cid,
                message,
                request_id: Some(request_id),
            })
        }
    };
    Some(HandledRequestResult { response, uuid })
}

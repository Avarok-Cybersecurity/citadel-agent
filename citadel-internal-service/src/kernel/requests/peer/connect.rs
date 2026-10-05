use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::InternalServiceRequest;
use citadel_sdk::logging::info;
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::PeerConnect {
        request_id,
        cid,
        peer_cid,
        udp_mode,
        session_security_settings,
        peer_session_password,
        turn,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };

    info!(target: "citadel", "[PeerConnect] *** RECEIVED PeerConnect REQUEST *** cid={}, peer_cid={}, request_id={:?}", cid, peer_cid, request_id);

    let response = super::dial::dial(
        this,
        super::dial::Dial {
            cid,
            peer_cid,
            udp_mode,
            session_security_settings,
            peer_session_password,
            turn,
            request_id: Some(request_id),
        },
    )
    .await;
    Some(HandledRequestResult { response, uuid })
}

use crate::kernel::requests::peer::{
    build_peer_information_map, peers_from_response, query_server_peer_list,
};
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, ListRegisteredPeersFailure,
    ListRegisteredPeersResponse,
};
use citadel_sdk::logging::{error, info};
use citadel_sdk::prelude::{ClientConnectionType, NodeResult, PeerEvent, PeerSignal, Ratchet};
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::ListRegisteredPeers { request_id, cid } = request else {
        unreachable!("Should never happen if programmed properly")
    };
    info!(
        "[ListRegisteredPeers] Handling request for cid={}, request_id={:?}",
        cid, request_id
    );
    let remote = this.remote();

    // Ask the server for this session's mutually-registered peers. We drive the
    // callback subscription ourselves (rather than the SDK's
    // `get_local_group_mutual_peers`) so that the server's `response: None`
    // reply — the normal "zero registered peers" answer for a new account — is
    // handled as an empty list instead of hanging the stream until a timeout.
    // See `query_server_peer_list` for the full rationale.
    let signal = PeerSignal::GetMutuals {
        v_conn_type: ClientConnectionType::Server { session_cid: cid },
        response: None,
    };
    let result = query_server_peer_list(remote, cid, signal, |event| {
        if let NodeResult::PeerEvent(PeerEvent {
            event: PeerSignal::GetMutuals { response, .. },
            ..
        }) = event
        {
            Some(peers_from_response(response))
        } else {
            None
        }
    })
    .await;

    match result {
        Ok(mutuals) => {
            info!(
                "[ListRegisteredPeers] SUCCESS: Found {} peers for cid={}",
                mutuals.len(),
                cid
            );
            let peers = build_peer_information_map(cid, mutuals, &this.peer_username_cache.read());
            let response =
                InternalServiceResponse::ListRegisteredPeersResponse(ListRegisteredPeersResponse {
                    cid,
                    peers,
                    request_id: Some(request_id),
                });
            Some(HandledRequestResult { response, uuid })
        }

        Err(err) => {
            let error_msg = err.into_string();
            error!(
                "[ListRegisteredPeers] FAILURE for cid={}: {}",
                cid, error_msg
            );
            let response =
                InternalServiceResponse::ListRegisteredPeersFailure(ListRegisteredPeersFailure {
                    cid,
                    message: error_msg,
                    request_id: Some(request_id),
                });
            Some(HandledRequestResult { response, uuid })
        }
    }
}

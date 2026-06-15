use crate::kernel::requests::peer::{
    build_peer_information_map, peers_from_response, query_server_peer_list,
};
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, ListAllPeersFailure, ListAllPeersResponse,
};
use citadel_sdk::logging::{error, info};
use citadel_sdk::prelude::{ClientConnectionType, NodeResult, PeerEvent, PeerSignal, Ratchet};
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::ListAllPeers { request_id, cid } = request else {
        unreachable!("Should never happen if programmed properly")
    };
    info!(
        "[ListAllPeers] Handling request for cid={}, request_id={:?}",
        cid, request_id
    );
    let remote = this.remote();

    // Ask the server for all local-group peers (registered or not). As with
    // ListRegisteredPeers, we drive the callback subscription ourselves rather
    // than the SDK's `get_local_group_peers` so the server's `response: None`
    // reply (no peers) returns an empty list immediately instead of hanging
    // until a timeout. See `query_server_peer_list` for the rationale.
    let signal = PeerSignal::GetRegisteredPeers {
        peer_conn_type: ClientConnectionType::Server { session_cid: cid },
        response: None,
        limit: None,
    };
    let result = query_server_peer_list(remote, cid, signal, |event| {
        if let NodeResult::PeerEvent(PeerEvent {
            event: PeerSignal::GetRegisteredPeers { response, .. },
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
        Ok(peers) => {
            info!(
                "[ListAllPeers] SUCCESS: Found {} peers for cid={}",
                peers.len(),
                cid
            );
            let peer_information =
                build_peer_information_map(cid, peers, &this.peer_username_cache.read());
            let response = InternalServiceResponse::ListAllPeersResponse(ListAllPeersResponse {
                cid,
                peer_information,
                request_id: Some(request_id),
            });
            Some(HandledRequestResult { response, uuid })
        }

        Err(err) => {
            let error_msg = err.into_string();
            error!("[ListAllPeers] FAILURE for cid={}: {}", cid, error_msg);
            let response = InternalServiceResponse::ListAllPeersFailure(ListAllPeersFailure {
                cid,
                message: error_msg,
                request_id: Some(request_id),
            });
            Some(HandledRequestResult { response, uuid })
        }
    }
}

use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, ListAllPeersFailure, ListAllPeersResponse,
    PeerInformation,
};
use citadel_sdk::logging::{error, info, warn};
use citadel_sdk::prelude::{ProtocolRemoteExt, Ratchet};
use std::time::Duration;
use tokio::time::timeout;
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

    info!(
        "[ListAllPeers] Calling get_local_group_peers for cid={}",
        cid
    );

    // Add 5 second timeout to prevent SDK call from hanging indefinitely
    // get_local_group_peers returns all peers on the network (may or may not be registered)
    let result = timeout(
        Duration::from_secs(5),
        remote.get_local_group_peers(cid, None),
    )
    .await;

    let sdk_result = match result {
        Ok(inner) => inner,
        Err(_) => {
            warn!("[ListAllPeers] TIMEOUT: get_local_group_peers took >5s for cid={}, returning empty list", cid);
            // Return empty list on timeout rather than error
            let peers = ListAllPeersResponse {
                cid,
                peer_information: std::collections::HashMap::new(),
                request_id: Some(request_id),
            };
            let response = InternalServiceResponse::ListAllPeersResponse(peers);
            return Some(HandledRequestResult { response, uuid });
        }
    };

    match sdk_result {
        Ok(peers) => {
            info!(
                "[ListAllPeers] SUCCESS: Found {} peers for cid={}",
                peers.len(),
                cid
            );
            let peer_information = peers
                .into_iter()
                .filter(|peer| peer.cid != cid) // Filter out self
                .map(|peer| {
                    (
                        peer.cid,
                        PeerInformation {
                            cid: peer.cid,
                            online_status: peer.is_online,
                            name: peer.full_name,
                            username: peer.username,
                        },
                    )
                })
                .collect();

            let response_payload = ListAllPeersResponse {
                cid,
                peer_information,
                request_id: Some(request_id),
            };

            let response = InternalServiceResponse::ListAllPeersResponse(response_payload);
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

use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, ListRegisteredPeersFailure,
    ListRegisteredPeersResponse, PeerInformation,
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
    let InternalServiceRequest::ListRegisteredPeers { request_id, cid } = request else {
        unreachable!("Should never happen if programmed properly")
    };
    info!("[ListRegisteredPeers] Handling request for cid={}, request_id={:?}", cid, request_id);
    let remote = this.remote();

    info!("[ListRegisteredPeers] Calling get_local_group_mutual_peers for cid={}", cid);

    // Add 5 second timeout to prevent SDK call from hanging indefinitely
    let result = timeout(
        Duration::from_secs(5),
        remote.get_local_group_mutual_peers(cid)
    ).await;

    let sdk_result = match result {
        Ok(inner) => inner,
        Err(_) => {
            warn!("[ListRegisteredPeers] TIMEOUT: get_local_group_mutual_peers took >5s for cid={}, returning empty list", cid);
            // Return empty list on timeout rather than error - this is likely due to no registered peers
            let peers = ListRegisteredPeersResponse {
                cid,
                peers: std::collections::HashMap::new(),
                request_id: Some(request_id),
            };
            let response = InternalServiceResponse::ListRegisteredPeersResponse(peers);
            return Some(HandledRequestResult { response, uuid });
        }
    };

    match sdk_result {
        Ok(peers) => {
            info!("[ListRegisteredPeers] SUCCESS: Found {} peers for cid={}", peers.len(), cid);
            let peers = ListRegisteredPeersResponse {
                cid,
                peers: peers
                    .clone()
                    .into_iter()
                    .filter(|peer| peer.cid != cid)
                    .map(|peer| {
                        (
                            peer.cid,
                            PeerInformation {
                                cid,
                                online_status: peer.is_online,
                                name: peer.full_name,
                                username: peer.username,
                            },
                        )
                    })
                    .collect(),
                request_id: Some(request_id),
            };

            let response = InternalServiceResponse::ListRegisteredPeersResponse(peers);
            Some(HandledRequestResult { response, uuid })
        }

        Err(err) => {
            let error_msg = err.into_string();
            error!("[ListRegisteredPeers] FAILURE for cid={}: {}", cid, error_msg);
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

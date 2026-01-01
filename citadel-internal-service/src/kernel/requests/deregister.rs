use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    DeregisterFailure, DeregisterSuccess, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::logging::info;
use citadel_sdk::prelude::{DeregisterFromHypernode, NodeRequest, Ratchet, VirtualTargetType};
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::Deregister { request_id, cid } = request else {
        unreachable!("Should never happen if programmed properly")
    };

    info!(target: "citadel", "Processing Deregister request for CID {cid}");

    let remote = this.remote();

    // Create the deregister request using the C2S connection type
    // This permanently removes the account from the server
    let request = NodeRequest::DeregisterFromHypernode(DeregisterFromHypernode {
        session_cid: cid,
        v_conn_type: VirtualTargetType::LocalGroupServer { session_cid: cid },
    });

    // Remove from connection map first
    this.server_connection_map.write().remove(&cid);

    match remote.send(request).await {
        Ok(_res) => {
            info!(target: "citadel", "Deregister successful for CID {cid}");
            let deregister_success =
                InternalServiceResponse::DeregisterSuccess(DeregisterSuccess {
                    cid,
                    request_id: Some(request_id),
                });
            Some(HandledRequestResult {
                response: deregister_success,
                uuid,
            })
        }
        Err(err) => {
            let error_message = format!("Failed to deregister: {err:?}");
            info!(target: "citadel", "{error_message}");
            let deregister_failure =
                InternalServiceResponse::DeregisterFailure(DeregisterFailure {
                    cid,
                    message: error_message,
                    request_id: Some(request_id),
                });
            Some(HandledRequestResult {
                response: deregister_failure,
                uuid,
            })
        }
    }
}

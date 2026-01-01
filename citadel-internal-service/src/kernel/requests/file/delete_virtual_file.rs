use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    DeleteVirtualFileFailure, DeleteVirtualFileSuccess, InternalServiceRequest,
    InternalServiceResponse,
};
use citadel_sdk::logging::error;
use citadel_sdk::prelude::{DeleteObject, NetworkError, NodeRequest, Ratchet, VirtualTargetType};
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::DeleteVirtualFile {
        virtual_directory,
        cid,
        peer_cid,
        request_id,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };
    let remote = this.remote();

    // Extract what we need from the lock, then drop it before any await
    let delete_request: Result<NodeRequest, NetworkError> = {
        let lock = this.server_connection_map.read();
        match lock.get(&cid) {
            Some(conn) => {
                if let Some(peer_cid) = peer_cid {
                    if conn.peers.contains_key(&peer_cid) {
                        Ok(NodeRequest::DeleteObject(DeleteObject {
                            v_conn: VirtualTargetType::LocalGroupPeer {
                                session_cid: cid,
                                peer_cid,
                            },
                            virtual_dir: virtual_directory,
                            security_level: Default::default(),
                        }))
                    } else {
                        Err(NetworkError::msg("Peer Connection Not Found"))
                    }
                } else {
                    Ok(NodeRequest::DeleteObject(DeleteObject {
                        v_conn: VirtualTargetType::LocalGroupServer { session_cid: cid },
                        virtual_dir: virtual_directory,
                        security_level: Default::default(),
                    }))
                }
            }
            None => {
                error!(target: "citadel","delete_virtual_file: server connection not found");
                Err(NetworkError::msg("delete_virtual_file: Server Connection Not Found"))
            }
        }
    }; // Lock dropped here - BEFORE any await

    let response = match delete_request {
        Ok(request) => {
            match remote.send(request).await {
                Ok(_) => {
                    InternalServiceResponse::DeleteVirtualFileSuccess(DeleteVirtualFileSuccess {
                        cid,
                        request_id: Some(request_id),
                    })
                }
                Err(err) => {
                    InternalServiceResponse::DeleteVirtualFileFailure(DeleteVirtualFileFailure {
                        cid,
                        message: err.into_string(),
                        request_id: Some(request_id),
                    })
                }
            }
        }
        Err(err) => {
            InternalServiceResponse::DeleteVirtualFileFailure(DeleteVirtualFileFailure {
                cid,
                message: err.into_string(),
                request_id: Some(request_id),
            })
        }
    };

    Some(HandledRequestResult { response, uuid })
}

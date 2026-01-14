use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, SendFileRequestFailure, SendFileRequestSuccess,
};
use citadel_sdk::logging::{error, info};
use citadel_sdk::prelude::{
    NetworkError, NodeRequest, Ratchet, SendObject, TargetLockedRemote, VirtualTargetType,
};
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::SendFile {
        request_id,
        source,
        cid,
        peer_cid,
        chunk_size,
        transfer_type,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };
    let remote = this.remote().clone();

    // Extract what we need from the lock, then drop it before any await
    let send_request: Result<NodeRequest, NetworkError> = {
        let lock = this.server_connection_map.read();
        match lock.get(&cid) {
            Some(conn) => {
                if let Some(peer_cid) = peer_cid {
                    if let Some(peer_conn) = conn.peers.get(&peer_cid) {
                        if let Some(peer_remote) = &peer_conn.remote {
                            Ok(NodeRequest::SendObject(SendObject {
                                source: Box::new(source),
                                chunk_size,
                                session_cid: cid,
                                v_conn_type: *peer_remote.user(),
                                transfer_type,
                            }))
                        } else {
                            Err(NetworkError::msg("Peer connection missing remote (acceptor-only connection cannot send files)"))
                        }
                    } else {
                        Err(NetworkError::msg("Peer Connection Not Found"))
                    }
                } else {
                    Ok(NodeRequest::SendObject(SendObject {
                        source: Box::new(source),
                        chunk_size,
                        session_cid: cid,
                        v_conn_type: VirtualTargetType::LocalGroupServer { session_cid: cid },
                        transfer_type,
                    }))
                }
            }
            None => {
                error!(target: "citadel","upload: server connection not found");
                Err(NetworkError::msg("upload: Server Connection Not Found"))
            }
        }
    }; // Lock dropped here - BEFORE any await

    match send_request {
        Ok(request) => {
            let result = remote.send(request).await;
            match result {
                Ok(_) => {
                    info!(target: "citadel","InternalServiceRequest Send File Success");
                    let response =
                        InternalServiceResponse::SendFileRequestSuccess(SendFileRequestSuccess {
                            cid,
                            request_id: Some(request_id),
                        });
                    Some(HandledRequestResult { response, uuid })
                }
                Err(err) => {
                    error!(target: "citadel","InternalServiceRequest Send File Failure");
                    let response =
                        InternalServiceResponse::SendFileRequestFailure(SendFileRequestFailure {
                            cid,
                            message: err.into_string(),
                            request_id: Some(request_id),
                        });
                    Some(HandledRequestResult { response, uuid })
                }
            }
        }
        Err(err) => {
            let response =
                InternalServiceResponse::SendFileRequestFailure(SendFileRequestFailure {
                    cid,
                    message: err.into_string(),
                    request_id: Some(request_id),
                });
            Some(HandledRequestResult { response, uuid })
        }
    }
}

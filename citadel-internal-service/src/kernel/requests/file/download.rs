use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    DownloadFileFailure, DownloadFileSuccess, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::logging::error;
use citadel_sdk::prelude::{NetworkError, NodeRequest, PullObject, Ratchet, TargetLockedRemote};
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::DownloadFile {
        virtual_directory,
        security_level,
        delete_on_pull,
        cid,
        peer_cid,
        request_id,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };
    let remote = this.remote();
    let security_level = security_level.unwrap_or_default();

    // Extract what we need from the lock, then drop it before any await
    let pull_request: Result<NodeRequest, NetworkError> = {
        let lock = this.server_connection_map.read();
        match lock.get(&cid) {
            Some(conn) => {
                if let Some(peer_cid) = peer_cid {
                    if let Some(peer_conn) = conn.peers.get(&peer_cid) {
                        if let Some(peer_remote) = &peer_conn.remote {
                            Ok(NodeRequest::PullObject(PullObject {
                                v_conn: *peer_remote.user(),
                                virtual_dir: virtual_directory,
                                delete_on_pull,
                                transfer_security_level: security_level,
                            }))
                        } else {
                            Err(NetworkError::msg("Peer connection missing remote (acceptor-only connection cannot download files)"))
                        }
                    } else {
                        Err(NetworkError::msg("Peer Connection Not Found"))
                    }
                } else {
                    Ok(NodeRequest::PullObject(PullObject {
                        v_conn: *conn.client_server_remote.user(),
                        virtual_dir: virtual_directory,
                        delete_on_pull,
                        transfer_security_level: security_level,
                    }))
                }
            }
            None => {
                error!(target: "citadel","download: server connection not found");
                Err(NetworkError::msg("download: Server Connection Not Found"))
            }
        }
    }; // Lock dropped here - BEFORE any await

    let response = match pull_request {
        Ok(request) => {
            match remote.send(request).await {
                Ok(_) => InternalServiceResponse::DownloadFileSuccess(DownloadFileSuccess {
                    cid,
                    request_id: Some(request_id),
                }),
                Err(err) => InternalServiceResponse::DownloadFileFailure(DownloadFileFailure {
                    cid,
                    message: err.into_string(),
                    request_id: Some(request_id),
                }),
            }
        }
        Err(err) => InternalServiceResponse::DownloadFileFailure(DownloadFileFailure {
            cid,
            message: err.into_string(),
            request_id: Some(request_id),
        }),
    };

    Some(HandledRequestResult { response, uuid })
}

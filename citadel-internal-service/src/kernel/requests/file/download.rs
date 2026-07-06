use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    DownloadFileFailure, DownloadFileSuccess, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::logging::error;
use citadel_sdk::prelude::{
    NetworkError, NodeRequest, PullObject, Ratchet, TargetLockedRemote, VirtualTargetType,
};
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
                    if conn.peers.contains_key(&peer_cid) {
                        // Construct the VirtualTargetType directly from the
                        // CID pair rather than dereferencing
                        // `peer_conn.remote` (which is `None` on the
                        // acceptor side, blocking acceptor-side downloads
                        // — same bug as the upload.rs fix in this PR, and
                        // matching the pattern already used by the sibling
                        // `delete_virtual_file.rs:36`). This makes
                        // download symmetric: both initiator and acceptor
                        // can pull files from the peer once the P2P
                        // channel is established.
                        Ok(NodeRequest::PullObject(PullObject {
                            v_conn: VirtualTargetType::LocalGroupPeer {
                                session_cid: cid,
                                peer_cid,
                            },
                            virtual_dir: virtual_directory,
                            delete_on_pull,
                            transfer_security_level: security_level,
                        }))
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
        Ok(request) => match remote.send(request).await {
            Ok(_) => InternalServiceResponse::DownloadFileSuccess(DownloadFileSuccess {
                cid,
                request_id: Some(request_id),
            }),
            Err(err) => InternalServiceResponse::DownloadFileFailure(DownloadFileFailure {
                cid,
                message: err.into_string(),
                request_id: Some(request_id),
            }),
        },
        Err(err) => InternalServiceResponse::DownloadFileFailure(DownloadFileFailure {
            cid,
            message: err.into_string(),
            request_id: Some(request_id),
        }),
    };

    Some(HandledRequestResult { response, uuid })
}

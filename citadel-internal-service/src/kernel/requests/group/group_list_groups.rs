use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    GroupListGroupsFailure, GroupListGroupsSuccess, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::prelude::{ProtocolRemoteTargetExt, Ratchet};
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::GroupListGroupsFor {
        cid,
        peer_cid,
        request_id,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };

    // Enum to hold either a peer remote or client-server remote
    enum RemoteType<R: Ratchet> {
        Peer(citadel_sdk::prelude::remote_specialization::PeerRemote<R>),
        Server(citadel_sdk::prefabs::ClientServerRemote<R>),
    }

    // Extract the remote inside lock block, then drop before await
    let remote_result: Result<RemoteType<R>, String> = {
        let server_connection_map = this.server_connection_map.read();
        match server_connection_map.get(&cid) {
            Some(connection) => {
                if let Some(peer_cid_val) = peer_cid {
                    match connection.peers.get(&peer_cid_val) {
                        Some(peer_connection) => match &peer_connection.remote {
                            Some(peer_remote) => Ok(RemoteType::Peer(peer_remote.clone())),
                            None => Err("Could Not List Groups - Peer connection missing remote (acceptor-only connection)".to_string()),
                        },
                        None => Err("Could Not List Groups - Peer not found".to_string()),
                    }
                } else {
                    Ok(RemoteType::Server(connection.client_server_remote.clone()))
                }
            }
            None => Err("Could Not List Groups - Connection not found".to_string()),
        }
    }; // Lock dropped here - BEFORE any await

    let response = match remote_result {
        Ok(remote_type) => {
            let result = match remote_type {
                RemoteType::Peer(remote) => remote.list_owned_groups().await,
                RemoteType::Server(remote) => remote.list_owned_groups().await,
            };
            match result {
                Ok(groups) => {
                    InternalServiceResponse::GroupListGroupsSuccess(GroupListGroupsSuccess {
                        cid,
                        peer_cid,
                        request_id: Some(request_id),
                        group_list: Some(groups),
                    })
                }
                Err(err) => {
                    InternalServiceResponse::GroupListGroupsFailure(GroupListGroupsFailure {
                        cid,
                        message: err.to_string(),
                        request_id: Some(request_id),
                    })
                }
            }
        }
        Err(message) => InternalServiceResponse::GroupListGroupsFailure(GroupListGroupsFailure {
            cid,
            message,
            request_id: Some(request_id),
        }),
    };

    Some(HandledRequestResult { response, uuid })
}

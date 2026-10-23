use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    GroupListGroupsFailure, GroupListGroupsSuccess, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::prefabs::ClientServerRemote;
use citadel_sdk::prelude::{
    MessageGroupKey, NetworkError, ProtocolRemoteExt, ProtocolRemoteTargetExt, Ratchet,
};
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

    // Only the connection's existence is checked under the lock. A peer's
    // groups are asked for through the registered-peer handle (find_target),
    // not the live peer connection's remote: ListGroupsFor is a
    // client-to-server command, and an acceptor-only (agent-answered)
    // connection carries no remote, which refused every such lister.
    let server_remote: Result<ClientServerRemote<R>, String> = {
        let server_connection_map = this.server_connection_map.read();
        match server_connection_map.get(&cid) {
            Some(connection) => Ok(connection.client_server_remote.clone()),
            None => Err("Could Not List Groups - Connection not found".to_string()),
        }
    }; // Lock dropped here - BEFORE any await

    let remote_result: Result<Result<Vec<MessageGroupKey>, NetworkError>, String> =
        match (server_remote, peer_cid) {
            (Err(message), _) => Err(message),
            (Ok(remote), None) => Ok(remote.list_owned_groups().await),
            (Ok(_), Some(peer_cid_val)) => match this.remote().find_target(cid, peer_cid_val).await
            {
                Ok(peer) => Ok(peer.list_owned_groups().await),
                Err(e) => Err(format!("Could Not List Groups - Peer not found: {e}")),
            },
        };

    let response = match remote_result {
        Ok(result) => match result {
            Ok(groups) => InternalServiceResponse::GroupListGroupsSuccess(GroupListGroupsSuccess {
                cid,
                peer_cid,
                request_id: Some(request_id),
                group_list: Some(groups),
            }),
            Err(err) => InternalServiceResponse::GroupListGroupsFailure(GroupListGroupsFailure {
                cid,
                message: err.to_string(),
                request_id: Some(request_id),
            }),
        },
        Err(message) => InternalServiceResponse::GroupListGroupsFailure(GroupListGroupsFailure {
            cid,
            message,
            request_id: Some(request_id),
        }),
    };

    Some(HandledRequestResult { response, uuid })
}

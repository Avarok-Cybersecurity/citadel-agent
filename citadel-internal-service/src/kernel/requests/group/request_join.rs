use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    GroupRequestJoinFailure, GroupRequestJoinSuccess, InternalServiceRequest,
    InternalServiceResponse,
};
use citadel_sdk::prelude::{
    GroupBroadcast, GroupBroadcastCommand, GroupEvent, NodeRequest, NodeResult, Ratchet,
    TargetLockedRemote,
};
use futures::StreamExt;
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::GroupRequestJoin {
        cid,
        group_key,
        request_id,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };

    // Extract peer_remote inside lock block, then drop before await
    let target_cid = group_key.cid;
    let peer_remote_result = {
        let server_connection_map = this.server_connection_map.read();
        match server_connection_map.get(&cid) {
            Some(connection) => match connection.peers.get(&target_cid) {
                Some(peer_connection) => match &peer_connection.remote {
                    Some(peer_remote) => Ok(peer_remote.clone()),
                    None => Err("Could not Request to join Group - Peer connection missing remote (acceptor-only connection)".to_string()),
                },
                None => Err("Could not Request to join Group - Peer not found".to_string()),
            },
            None => Err("Could not Request to join Group - Connection not found".to_string()),
        }
    }; // Lock dropped here - BEFORE any await

    let response = match peer_remote_result {
        Ok(peer_remote) => {
            let group_request = GroupBroadcast::RequestJoin {
                sender: cid,
                key: group_key,
            };
            let request = NodeRequest::GroupBroadcastCommand(GroupBroadcastCommand {
                session_cid: cid,
                command: group_request,
            });
            match peer_remote
                .remote()
                .send_callback_subscription(request)
                .await
            {
                Ok(mut subscription) => {
                    let mut result = Err("Group Request Join Failed".to_string());
                    while let Some(evt) = subscription.next().await {
                        if let NodeResult::GroupEvent(GroupEvent {
                            session_cid: _,
                            ticket: _,
                            event:
                                GroupBroadcast::RequestJoinPending {
                                    result: signal_result,
                                    key: _key,
                                },
                        }) = evt
                        {
                            result = signal_result;
                            break;
                        }
                    }
                    match result {
                        Ok(_) => InternalServiceResponse::GroupRequestJoinSuccess(
                            GroupRequestJoinSuccess {
                                cid,
                                group_key,
                                request_id: Some(request_id),
                            },
                        ),
                        Err(err) => InternalServiceResponse::GroupRequestJoinFailure(
                            GroupRequestJoinFailure {
                                cid,
                                message: err.to_string(),
                                request_id: Some(request_id),
                            },
                        ),
                    }
                }
                Err(err) => {
                    InternalServiceResponse::GroupRequestJoinFailure(GroupRequestJoinFailure {
                        cid,
                        message: err.to_string(),
                        request_id: Some(request_id),
                    })
                }
            }
        }
        Err(message) => InternalServiceResponse::GroupRequestJoinFailure(GroupRequestJoinFailure {
            cid,
            message,
            request_id: Some(request_id),
        }),
    };

    Some(HandledRequestResult { response, uuid })
}

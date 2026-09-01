use super::request_join_wait::{await_join_outcome, JoinOutcome, GROUP_REQUEST_JOIN_WAIT};
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    GroupRequestJoinFailure, GroupRequestJoinSuccess, InternalServiceRequest,
    InternalServiceResponse,
};
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::{
    GroupBroadcast, GroupBroadcastCommand, NodeRequest, Ratchet, TargetLockedRemote,
};
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
                    // Bounded, and matching every terminal variant — see
                    // request_join_wait. The loop here named only
                    // RequestJoinPending and re-awaited a subscription that
                    // never ends on the other two.
                    let result = match await_join_outcome(
                        &mut subscription,
                        GROUP_REQUEST_JOIN_WAIT,
                    )
                    .await
                    {
                        JoinOutcome::Pending(signal_result) => signal_result,
                        JoinOutcome::Answered(true) => Ok(()),
                        JoinOutcome::Answered(false) => {
                            Err("The group refused the join request".to_string())
                        }
                        JoinOutcome::GroupGone => Err("That group no longer exists".to_string()),
                        JoinOutcome::Ended => Err("Group Request Join Failed".to_string()),
                        JoinOutcome::TimedOut => {
                            warn!(target: "citadel", "Group join request for CID {cid} received no answer within {GROUP_REQUEST_JOIN_WAIT:?}; the group owner may be offline");
                            Err(format!(
                                "No answer to the join request within {GROUP_REQUEST_JOIN_WAIT:?}"
                            ))
                        }
                    };
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

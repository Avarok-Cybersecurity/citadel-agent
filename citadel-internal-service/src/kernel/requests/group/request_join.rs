use super::request_join_wait::{await_join_outcome, JoinOutcome, GROUP_REQUEST_JOIN_WAIT};
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    GroupRequestJoinFailure, GroupRequestJoinSuccess, InternalServiceRequest,
    InternalServiceResponse,
};
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::{GroupBroadcast, GroupBroadcastCommand, NodeRequest, Ratchet};
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

    // RequestJoin is a client-to-server broadcast command: the SERVER owns
    // group membership and forwards the ask to the group's owner, so it goes
    // out on the node remote, like respond_request.rs and create.rs. Requiring
    // a peer remote to the owner refused every joiner whose P2P connection to
    // the owner was ACCEPTED rather than dialled -- an acceptor-only (and every
    // agent-answered) connection carries no remote -- before any request left.
    let remote_result = {
        let server_connection_map = this.server_connection_map.read();
        match server_connection_map.get(&cid) {
            Some(_connection) => Ok(this.remote().clone()),
            None => Err("Could not Request to join Group - Connection not found".to_string()),
        }
    }; // Lock dropped here - BEFORE any await

    let response = match remote_result {
        Ok(remote) => {
            let group_request = GroupBroadcast::RequestJoin {
                sender: cid,
                key: group_key,
            };
            let request = NodeRequest::GroupBroadcastCommand(GroupBroadcastCommand {
                session_cid: cid,
                command: group_request,
            });
            match remote.send_callback_subscription(request).await {
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

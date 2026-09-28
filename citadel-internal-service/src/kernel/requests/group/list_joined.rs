use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    GroupListJoinedFailure, GroupListJoinedSuccess, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

/// The groups this session is in, from its own live group channels, and the invitations it has
/// not answered (kernel/pending_group_invites.rs).
///
/// No round trip to the server: the channels ARE the membership this agent acts
/// on -- a message to a group not listed here is refused -- and they are filled
/// on (re)connect by the server's RestoreOwnership / RestoreMembership, so a
/// browser that has never seen the session learns every group it is in.
pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::GroupListJoined { cid, request_id } = request else {
        unreachable!("Should never happen if programmed properly")
    };

    let listed = this
        .server_connection_map
        .read()
        .get(&cid)
        .map(|connection| {
            let groups = connection.groups.keys();
            let pending_invites = connection.pending_group_invites.unanswered(&groups);
            (groups, pending_invites)
        });

    let response = match listed {
        Some((groups, pending_invites)) => {
            InternalServiceResponse::GroupListJoinedSuccess(GroupListJoinedSuccess {
                cid,
                groups,
                request_id: Some(request_id),
                pending_invites: Some(pending_invites),
            })
        }
        None => InternalServiceResponse::GroupListJoinedFailure(GroupListJoinedFailure {
            cid,
            message: "Could Not List Groups - Connection not found".to_string(),
            request_id: Some(request_id),
        }),
    };

    Some(HandledRequestResult { response, uuid })
}

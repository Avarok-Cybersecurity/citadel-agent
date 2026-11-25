//! The server's answers to this session's own group requests, for the window: a message's
//! delivery report or refusal, a leave it took, an add or kick it refused, the group passing to
//! a new owner, and the group restored to this device. Each reaches the agent by the group channel or as a kernel `GroupEvent`, and both build
//! it here (`answer`); none is dropped silently.
//!
//! A message's report or refusal carries the request it answers when the group's channel paired
//! them (kernel/group_sends.rs); every other answer, and one that reached no channel, carries
//! `None`.

use citadel_internal_service_types::{
    GroupInviteFailure, GroupKickFailure, GroupLeaveNotification, GroupMessageRefusedNotification,
    GroupMessageResponse, GroupOwnershipTransferredNotification, GroupRestoredNotification,
    InternalServiceResponse,
};
use citadel_sdk::prelude::{GroupBroadcast, MessageGroupKey};

/// What the window is told of `broadcast`, if it is one of the answers this module builds.
pub(crate) fn answer(
    cid: u64,
    broadcast: &GroupBroadcast,
    send: Option<uuid::Uuid>,
) -> Option<InternalServiceResponse> {
    match broadcast {
        GroupBroadcast::MessageResponse {
            key,
            delivery,
            id: _,
            position: _,
        } => Some(InternalServiceResponse::GroupMessageResponse(
            GroupMessageResponse {
                cid,
                group_key: *key,
                delivery: delivery.clone(),
                request_id: send,
            },
        )),
        GroupBroadcast::MessageRefused { key, refusal } => {
            Some(InternalServiceResponse::GroupMessageRefusedNotification(
                GroupMessageRefusedNotification {
                    cid,
                    group_key: *key,
                    refusal: *refusal,
                    request_id: send,
                },
            ))
        }
        GroupBroadcast::OwnershipTransferred {
            key,
            owner,
            previous,
            tree: _,
        } => Some(
            InternalServiceResponse::GroupOwnershipTransferredNotification(
                GroupOwnershipTransferredNotification {
                    cid,
                    group_key: *key,
                    owner: *owner,
                    previous: *previous,
                    request_id: None,
                },
            ),
        ),
        // A refused leave (the session is not in the group) is not reported.
        GroupBroadcast::LeaveRoomResponse {
            key,
            success: true,
            message,
        } => Some(InternalServiceResponse::GroupLeaveNotification(
            GroupLeaveNotification {
                cid,
                group_key: *key,
                success: true,
                message: message.clone(),
                request_id: None,
            },
        )),
        GroupBroadcast::Restored { key, source } => Some(
            InternalServiceResponse::GroupRestoredNotification(GroupRestoredNotification {
                cid,
                group_key: *key,
                source: *source,
                request_id: None,
            }),
        ),
        GroupBroadcast::KickResponse {
            key,
            success: false,
        } => Some(InternalServiceResponse::GroupKickFailure(
            GroupKickFailure {
                cid,
                message: refused("kick", *key),
                request_id: None,
            },
        )),
        GroupBroadcast::AddResponse {
            key,
            failed_to_invite_list: Some(failed),
        } if !failed.is_empty() => Some(InternalServiceResponse::GroupInviteFailure(
            GroupInviteFailure {
                cid,
                message: format!("{}: {}", refused("invitation", *key), cids(failed)),
                request_id: None,
            },
        )),
        _ => None,
    }
}

fn refused(operation: &str, key: MessageGroupKey) -> String {
    format!("The server refused the {operation} in group {key}")
}

fn cids(failed: &[u64]) -> String {
    let named: Vec<String> = failed.iter().map(u64::to_string).collect();
    format!("not invited: {}", named.join(", "))
}

#[cfg(test)]
#[path = "group_answers_tests.rs"]
mod tests;

//! Group traffic that reaches this session as a kernel `GroupEvent`, which is how the SDK hands
//! over a group's traffic when this node holds no channel for it: a channel dropped on a
//! reconnect or never adopted. Dropping a channel keeps the member in the group, so its traffic is
//! still this session's to read. Only a group this session has left (`GroupChannels::has_left`)
//! is silent.

use crate::kernel::group_channels::GroupChannels;
use citadel_internal_service_types::{
    GroupDisconnectNotification, GroupMessageNotification, InternalServiceResponse,
};
use citadel_sdk::prelude::{GroupEndReason, MessageGroupKey};

/// A message to a group this session is still in, whether or not it holds the group's channel.
pub(crate) fn message(
    groups: &GroupChannels,
    cid: u64,
    group_key: MessageGroupKey,
    peer_cid: u64,
    message: Vec<u8>,
) -> Option<InternalServiceResponse> {
    (!groups.has_left(&group_key)).then_some(InternalServiceResponse::GroupMessageNotification(
        GroupMessageNotification {
            cid,
            peer_cid,
            message,
            group_key,
            request_id: None,
        },
    ))
}

/// The group ended for this session. Told once: the entry is marked departed, so the same end
/// arriving again by the group's channel is not told twice.
pub(crate) fn disconnected(
    groups: &mut GroupChannels,
    cid: u64,
    group_key: MessageGroupKey,
    reason: GroupEndReason,
) -> Option<InternalServiceResponse> {
    let already_told = groups.has_left(&group_key);
    let _ = groups.mark_departed(&group_key);
    (!already_told).then_some(InternalServiceResponse::GroupDisconnectNotification(
        GroupDisconnectNotification {
            cid,
            group_key,
            reason,
            request_id: None,
        },
    ))
}

#[cfg(test)]
#[path = "group_heard_tests.rs"]
mod tests;

//! What became of one group message, member by member, as the sender's server reports it; who
//! owns a group (the group passing to a new owner, and the owner of each group listed); and
//! where a device's state for a group came from when the group was restored to it.
//!
//! The report is the SDK's own `GroupDelivery`, carried as it is: its TypeScript comes from
//! `@avarok/citadel-protocol-types`, so no copy of it lives here.

use citadel_types::proto::{GroupDelivery, GroupStateSource};
use citadel_types::prelude::MessageGroupKey;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// The server's account of a group message this session sent. `request_id` is the
/// `GroupMessage` it answers, which follows that request's `GroupMessageSuccess`; `None` when
/// the group's channel was gone by the time the answer came.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupMessageResponse {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "MessageGroupKey"))]
    pub group_key: MessageGroupKey,
    #[cfg_attr(feature = "typescript", ts(type = "GroupDelivery"))]
    pub delivery: GroupDelivery,
    pub request_id: Option<Uuid>,
}

/// The group's owner left, and `owner` owns it from now on. The group keeps its key, so
/// `group_key.cid` keeps naming its creator: the owner is the one named here.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupOwnershipTransferredNotification {
    /// The member session told.
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "MessageGroupKey"))]
    pub group_key: MessageGroupKey,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub owner: u64,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub previous: u64,
    pub request_id: Option<Uuid>,
}

/// Who owns `group_key`, as the server last reported it.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct JoinedGroupOwner {
    #[cfg_attr(feature = "typescript", ts(type = "MessageGroupKey"))]
    pub group_key: MessageGroupKey,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub owner: u64,
}

/// The group is live again for this session, and `source` says what it can read: everything
/// it read before (`ThisDevice`), what the account read before, from the state the server keeps
/// sealed for it (`Backup`, a recovered device), or only what is sent from now on (`Rejoined`,
/// `Refounded`).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupRestoredNotification {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "MessageGroupKey"))]
    pub group_key: MessageGroupKey,
    #[cfg_attr(feature = "typescript", ts(type = "GroupStateSource"))]
    pub source: GroupStateSource,
    pub request_id: Option<Uuid>,
}

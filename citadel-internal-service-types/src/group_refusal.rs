//! A group message the server refused to relay, which therefore reached nobody.
//!
//! A group can carry server-enforced caps (an app topic's manifest `limits.topics`), and only a
//! member may send to it. A message that breaks either is refused by the server with the SDK's
//! typed reason (`GroupMessageRefusal`), which the agent passes on to the sender's window as
//! this, so the sender learns it was not delivered and why.
//!
//! `request_id` is the `GroupMessage` it answers, as in `GroupMessageResponse`.

use citadel_types::prelude::MessageGroupKey;
use citadel_types::proto::GroupMessageRefusal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupMessageRefusedNotification {
    /// The session that sent the message.
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "MessageGroupKey"))]
    pub group_key: MessageGroupKey,
    #[cfg_attr(feature = "typescript", ts(type = "GroupMessageRefusal"))]
    pub refusal: GroupMessageRefusal,
    pub request_id: Option<Uuid>,
}

//! A group message the server refused to relay, which therefore reached nobody.
//!
//! A group can carry server-enforced caps (an app topic's manifest `limits.topics`). A message
//! that breaks one is refused by the server with a typed reason, which the agent passes on to
//! the sender's window as this, so the sender learns it was not delivered and why.
//!
//! It answers no request, so `request_id` is always `None`.

use citadel_types::prelude::MessageGroupKey;
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
    pub refusal: GroupMessageRefusalReason,
    pub request_id: Option<Uuid>,
}

/// Which of the group's caps the message broke.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum GroupMessageRefusalReason {
    /// The message was `bytes` long; the group allows `max_bytes`.
    TooLarge {
        #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
        bytes: u64,
        max_bytes: u32,
    },
    /// The sender had already sent `max_per_second` messages to the group this second.
    RateLimited { max_per_second: u32 },
}

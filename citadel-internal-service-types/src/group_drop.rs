//! A group message that reached this session but could not be read here.
//!
//! Group messages are end-to-end encrypted under key state that lives only in the SDK
//! session. A session that was just (re)established has none until the group's owner
//! re-adds it, and the relay keeps forwarding the group's ciphertext meanwhile. The SDK
//! used to drop those messages with a log line, so neither end ever learned of the loss;
//! it now reports each one, and the agent passes the report on as this.
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
pub struct GroupMessageDroppedNotification {
    /// The session the message was addressed to.
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "MessageGroupKey"))]
    pub group_key: MessageGroupKey,
    /// The CID of the member who sent the message.
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub sender: u64,
    /// Why this session could not read it, as the SDK put it.
    pub reason: String,
    pub request_id: Option<Uuid>,
}

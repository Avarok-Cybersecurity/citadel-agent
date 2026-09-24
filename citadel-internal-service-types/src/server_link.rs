//! What the agent tells a UI about a session's link to its workspace server.
//!
//! A server can drop the link without anyone asking — a deploy resets its WebSocket —
//! and the agent reconnects the session itself, under the same CID. These report that:
//! `ServerConnectionLost` when the link goes, then exactly one of `ServerReconnected`
//! or `ServerReconnectFailed`. After a failure the session is gone, and the usual
//! `DisconnectNotification` follows.
//!
//! None of them answers a request, so `request_id` is always `None`; it is here because
//! every response carries one.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ServerConnectionLost {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    /// Whether the agent is trying to get the link back.
    pub reconnecting: bool,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ServerReconnected {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ServerReconnectFailed {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub reason: String,
    pub request_id: Option<Uuid>,
}

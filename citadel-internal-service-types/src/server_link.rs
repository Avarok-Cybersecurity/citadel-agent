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
//!
//! Those three reach only the UI attached when they are sent. A give-up while no page is
//! open (a closed laptop lid, a tab closed during the outage) used to leave nothing but an
//! absence: the next `GetSessions` listed no session and no reason. `SignedOutSession` is
//! that give-up kept, reported in every `GetSessions` until the account signs in again.

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

/// A session the agent removed because its server would not take it back, kept so a UI that
/// was not attached at the time can say "signed out by the server, sign in again".
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct SignedOutSession {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub username: String,
    /// What `ServerReconnectFailed` said.
    pub reason: String,
}

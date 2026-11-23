//! An app's `citadel.session` (CW-6, `citadel-session-abi.md` §3): a C2S session to an external
//! Citadel server, held by the member's agent under an account the agent owns.
//!
//! The page asks with `AppSessionOpen`, `AppSessionSend` and `AppSessionClose`; the agent
//! answers and reports with `AppSessionState`, `AppSessionMessage` and `AppSessionFailure`.
//! `cid` is always the member's workspace session (the routing key, as in `AppFetch*`);
//! `server_cid` is the agent-held account's CID on the external server.

use crate::plaintext_debug_fmt;
use custom_debug::Debug;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

use citadel_app_sdk_types::calls::session;
/// Where an app session stands: the guest ABI's type, which this wire carries unchanged.
pub use citadel_app_sdk_types::calls::SessionStatus;

/// The most one `AppSessionSend` may carry, whatever the install's own limit says.
pub const APP_SESSION_MAX_BODY_BYTES: usize = session::SESSION_MAX_MSG_BYTES_CEILING as usize;
/// The most servers one install may hold sessions to for one member.
pub const APP_SESSION_MAX_SERVERS: usize = session::MAX_SESSION_SERVERS;

/// The answer to `AppSessionOpen` and `AppSessionClose`, and every later change of the
/// session's link (`request_id: None`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
#[serde(deny_unknown_fields)]
pub struct AppSessionState {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub install_id: String,
    pub session: u32,
    /// 0 until the agent-held account has signed in for the first time.
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub server_cid: u64,
    pub state: SessionStatus,
}

/// One message the external server sent the session.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
#[serde(deny_unknown_fields)]
pub struct AppSessionMessage {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub install_id: String,
    pub session: u32,
    #[cfg_attr(feature = "typescript", ts(type = "number[]"))]
    #[debug(with = plaintext_debug_fmt)]
    pub body: Vec<u8>,
}

/// A request the agent refused or could not complete. `reason` is one of
/// [`app_session_reason`]'s names.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
#[serde(deny_unknown_fields)]
pub struct AppSessionFailure {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub install_id: String,
    pub reason: String,
}

/// The names `AppSessionFailure.reason` takes.
pub mod app_session_reason {
    /// The workspace server did not grant this install `citadel.session` to that server.
    pub const UNGRANTED: &str = "ungranted";
    /// Over a size or count limit: the body, or the servers per install.
    pub const LIMIT: &str = "limit";
    /// No such session for this install, or it is closed; or a server that is not a name.
    pub const INVALID: &str = "invalid";
    /// The member signed in without a portable identity: the guest falls back to `net.host`.
    pub const NO_IDENTITY: &str = "no_identity";
    /// The member's identity is locked on this agent.
    pub const IDENTITY_LOCKED: &str = "identity_locked";
    /// This agent was started without app egress or without an identity.
    pub const NOT_CONFIGURED: &str = "not_configured";
    /// The caller is not attached to the member session it named.
    pub const SESSION: &str = "session";
    /// The workspace server did not say what the install may reach.
    pub const AUTHORITY: &str = "authority";
    /// Allowed, and the external server could not be registered with, signed in to or sent to.
    pub const UNAVAILABLE: &str = "unavailable";
}

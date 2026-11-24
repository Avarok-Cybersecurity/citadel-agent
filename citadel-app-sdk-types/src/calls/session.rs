//! `session` (`citadel.session`): a C2S session to an external Citadel server named in the
//! manifest's `servers`, held by the member's agent under an account the agent owns. The guest
//! opens it by server name and then sends and receives opaque bodies on its `session` number;
//! what arrives comes as `AppEvent::Session`, which needs the `events` subscription as topics do.

use crate::Bytes;
use alloc::string::String;

/// A manifest names at most this many `servers` for `citadel.session`.
pub const MAX_SESSION_SERVERS: usize = 2;
/// The ceiling on a manifest's `limits.session.max_msg_bytes`, and the most one body may carry.
pub const SESSION_MAX_MSG_BYTES_CEILING: u32 = 1_048_576;
/// The ceiling on a manifest's `limits.session.max_msgs_per_s`.
pub const SESSION_MAX_MSGS_PER_S_CEILING: u32 = 64;
/// `Refused::Limit` when a send exceeds the install's `max_msg_bytes`.
pub const LIMIT_SESSION_BYTES: &str = "session_bytes";
/// `Refused::Limit` when a send exceeds the install's message rate.
pub const LIMIT_SESSION_RATE: &str = "session_rate";

abi_types! {
    /// Open (or return the already open) session to `server`, a canonical domain in the
    /// manifest's `servers`. Reply `SessionState`.
    pub struct SessionOpen {
        pub server: String,
    }

    /// One body to the server. A size or rate refusal is answered at once.
    pub struct SessionSend {
        pub session: u32,
        pub body: Bytes,
    }

    pub struct SessionClose {
        pub session: u32,
    }

    /// `cid` is the agent-held account's CID on the external server.
    pub struct SessionState {
        pub session: u32,
        pub cid: u64,
        pub state: SessionStatus,
    }

    /// Where a session stands. The agent's wire (`AppSessionState.state`) carries this type.
    #[derive(Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum SessionStatus {
        Connecting,
        Connected,
        Closed { reason: String },
    }

    /// What happened on an open session.
    #[serde(rename_all = "snake_case")]
    pub enum SessionEvent {
        Message { body: Bytes },
        State { state: SessionStatus },
    }
}

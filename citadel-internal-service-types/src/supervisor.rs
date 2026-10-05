//! What the agent's connection supervisor tells a session's windows.
//!
//! The supervisor keeps an account's links alive with no window open (agent
//! `kernel/supervisor`). Windows only watch: these notices drive the connection status, and
//! `ConfigCommand::Interest` is how an open chat or call asks for its peer to be kept
//! connected.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// Where the supervisor has got to, for the account or, with a `peer_cid`, for one peer.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum SupervisorState {
    /// Being brought back: the server link, or the dial to the peer.
    Healing,
    /// Back.
    Healed,
    /// An attempt failed; the agent tries again after its backoff.
    Degraded,
}

/// Unsolicited: route it by `cid`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct SupervisorNotification {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    /// The peer it is about; absent when it is about the account's server link.
    #[cfg_attr(feature = "typescript", ts(type = "bigint | null"))]
    pub peer_cid: Option<u64>,
    pub state: SupervisorState,
    /// Always `None`: supervisor events are not answers to a request.
    pub request_id: Option<Uuid>,
}

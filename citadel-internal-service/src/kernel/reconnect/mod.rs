//! Bringing a session back when its server drops it without anyone asking.
//!
//! A deploy of the workspace server resets its WebSocket ("Connection reset without
//! closing handshake"). The session used to be removed on that, so every signed-in user
//! was signed out by a deploy. Now the session stays in the map under its CID, marked
//! `Reconnecting`, and the agent connects it again with what it was opened with; the UI
//! hears `ServerConnectionLost`, then `ServerReconnected` or `ServerReconnectFailed`.
//!
//! Only a user's Disconnect or Deregister ends a session, as before. Both take the entry
//! out of the map (Deregister marks it `Ending` first), and the reconnect checks for
//! that before each attempt and again before installing the new channel.
//!
//! The password is kept for this, in memory only, as a `SecBuffer`: locked, zeroed on
//! drop, and printed as `***SECRET***`. It lives on the `Connection`, so it goes when the
//! session does. `Credentials` deliberately has no `Debug`.
//!
//! policy.rs decides (pure, tested); task.rs does the SDK I/O.

pub(crate) mod policy;
#[cfg(test)]
mod policy_tests;
mod report;
pub(crate) mod task;

use citadel_sdk::prelude::{
    ConnectMode, PreSharedKey, SecBuffer, SessionSecuritySettings, UdpMode,
};
use std::time::Duration;
use uuid::Uuid;

/// Where a session's link to its server stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    Up,
    /// The server dropped it and the agent is connecting it again.
    Reconnecting,
    /// The user is ending it; a drop now is expected.
    Ending,
}

/// What a session was opened with, so it can be opened again the same way. The
/// username and server are the `Connection`'s own; the SDK dials the server recorded
/// for the account, so a hosted workspace is reached over its WebSocket URL again.
#[derive(Clone)]
pub struct Credentials {
    pub password: SecBuffer,
    pub connect_mode: ConnectMode,
    pub udp_mode: UdpMode,
    pub keep_alive_timeout: Option<Duration>,
    pub session_security_settings: SessionSecuritySettings,
    pub server_password: Option<PreSharedKey>,
    /// The Connect that opened the session; its notifications keep carrying it.
    pub connect_request_id: Uuid,
}

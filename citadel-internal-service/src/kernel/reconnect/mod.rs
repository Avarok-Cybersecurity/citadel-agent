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

mod attempt;
mod link;
mod lost_peers;
pub(crate) mod policy;

/// The log target of a session's link to its server: the drop, each attempt, the
/// outcome. Under `citadel`, so a `citadel=` filter still covers it, and its own so a
/// build that records only errors can record this path alone
/// (`RUST_LOG=error,citadel::reconnect=info`): a few lines per drop, and without them a
/// reconnect that failed says nothing about why.
pub const LOG_TARGET: &str = "citadel::reconnect";
#[cfg(test)]
mod policy_tests;
mod report;
pub(crate) mod sign_in;
pub(crate) mod signed_out;
#[cfg(test)]
mod stale_session_tests;
pub(crate) mod takeover;
pub(crate) mod task;

use citadel_sdk::prelude::{
    ConnectMode, PreSharedKey, SecBuffer, SessionSecuritySettings, UdpMode,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
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
    /// A user's sign-in is taking a reconnect over (reconnect/takeover.rs). The reconnect
    /// stops at its next check, and the sign-in owns the outcome: nothing else installs a
    /// link into the entry or removes it for a failed attempt.
    SigningIn,
}

/// What lets a sign-in stop the reconnect without ever running a second SDK connect for
/// the account beside it.
///
/// The reconnect holds `attempt` for the whole of each attempt, from checking that it is
/// still wanted to installing or discarding what the attempt produced; a takeover marks the
/// entry `SigningIn` first and then takes `attempt`, so it waits out an attempt in flight
/// and every later check sees the mark. `generation` names the reconnect run that is
/// wanted: a takeover that fails hands the session back to a NEW run, and an older one,
/// asleep between attempts, sees the number moved and stops instead of running beside it.
///
/// One pointer wide: it lives on every `Connection`, whose size several enums carry.
#[derive(Clone, Default)]
pub struct Handoff(Arc<HandoffState>);

#[derive(Default)]
struct HandoffState {
    attempt: tokio::sync::Mutex<()>,
    generation: AtomicU64,
}

impl Handoff {
    /// Wait for no attempt to be in flight, and keep it so while the guard lives.
    pub(crate) async fn hold_attempts(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.0.attempt.lock().await
    }

    pub(crate) fn generation(&self) -> u64 {
        self.0.generation.load(Ordering::SeqCst)
    }

    /// Start a new reconnect run; any older one stops at its next check. Called under
    /// the map's write lock, like every change of the link state it goes with.
    pub(crate) fn next_generation(&self) -> u64 {
        self.0
            .generation
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1)
    }
}

/// How a session can be signed in to again without its user (kernel/sign_in/mod.rs decides).
#[derive(Clone)]
pub enum Reauth {
    /// The password alone opened it.
    Password(SecBuffer),
    /// It needed the user, a security-key touch or a recovery code, which a reconnect cannot
    /// ask for unprompted: the reconnect gives up with this reason, and the user signs in.
    NeedsUser(&'static str),
}

/// What a session was opened with, so it can be opened again the same way. The
/// username and server are the `Connection`'s own; the SDK dials the server recorded
/// for the account, so a hosted workspace is reached over its WebSocket URL again.
#[derive(Clone)]
pub struct Credentials {
    pub reauth: Reauth,
    pub connect_mode: ConnectMode,
    pub udp_mode: UdpMode,
    pub keep_alive_timeout: Option<Duration>,
    pub session_security_settings: SessionSecuritySettings,
    pub server_password: Option<PreSharedKey>,
    /// The Connect that opened the session; its notifications keep carrying it.
    pub connect_request_id: Uuid,
}

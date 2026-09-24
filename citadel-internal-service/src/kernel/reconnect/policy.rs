//! When to try again, when to stop, and which failures end it at once. Pure: no clock,
//! no SDK, so every decision is tested directly (see policy_tests.rs).

use crate::kernel::reconnect::LinkState;
use citadel_io::ErrorCode;
use std::time::Duration;

/// How a session whose server dropped it is brought back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectPolicy {
    /// The wait before the first attempt, doubled after each failure.
    pub first_delay: Duration,
    pub max_delay: Duration,
    /// Measured from the drop. An attempt that could only start after it is not made.
    pub give_up_after: Duration,
    /// One attempt that has not answered by then counts as a transient failure.
    pub attempt_timeout: Duration,
}

/// The policy the agent runs. A deploy resets every socket at once and is back in
/// seconds; ten minutes covers a slow one without keeping a session up forever.
pub const SERVER_RECONNECT: ReconnectPolicy = ReconnectPolicy {
    first_delay: Duration::from_millis(500),
    max_delay: Duration::from_secs(30),
    give_up_after: Duration::from_secs(600),
    attempt_timeout: Duration::from_secs(30),
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// The server will say the same thing next time: wrong password, no such account.
    Refused,
    /// The server is unreachable or not ready yet.
    Transient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    RetryAfter(Duration),
    GiveUp(GiveUp),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GiveUp {
    Refused,
    OutOfTime,
}

/// What an SDK report that the server dropped a session means for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropAction {
    /// Nobody asked for this: keep the session and bring it back.
    Reconnect,
    /// A reconnect is already running; its own failed attempts report drops too.
    AlreadyReconnecting,
    /// The user is ending the session (deregistering); remove it as before.
    Remove,
}

pub fn on_unrequested_drop(link: LinkState) -> DropAction {
    match link {
        LinkState::Up => DropAction::Reconnect,
        LinkState::Reconnecting => DropAction::AlreadyReconnecting,
        LinkState::Ending => DropAction::Remove,
    }
}

impl ReconnectPolicy {
    /// The wait before attempt `attempt` (0 is the first).
    pub fn delay_before(&self, attempt: u32) -> Duration {
        let factor = 1u32.checked_shl(attempt).unwrap_or(u32::MAX);
        self.first_delay
            .checked_mul(factor)
            .map_or(self.max_delay, |delay| delay.min(self.max_delay))
    }

    /// After attempt `attempt` failed with `kind`, `elapsed` since the drop.
    pub fn after_failure(&self, attempt: u32, elapsed: Duration, kind: FailureKind) -> Next {
        if kind == FailureKind::Refused {
            return Next::GiveUp(GiveUp::Refused);
        }
        let delay = self.delay_before(attempt.saturating_add(1));
        if elapsed.saturating_add(delay) > self.give_up_after {
            return Next::GiveUp(GiveUp::OutOfTime);
        }
        Next::RetryAfter(delay)
    }
}

/// The account errors a retry cannot fix. From the SDK's registry, not copied strings.
const REFUSALS: [ErrorCode; 5] = [
    ErrorCode::AccountClientNonExists,
    ErrorCode::AccountServerNonExists,
    ErrorCode::AccountInvalidUsername,
    ErrorCode::AccountInvalidPassword,
    ErrorCode::AccountDisengaged,
];

/// Whether a failed connect is worth repeating.
///
/// A refusal raised on this side keeps its code. One the SERVER sends arrives as
/// `RemoteConnectFailed` carrying the server's rendered error, so it is recognised by
/// the leading text of the same registry entry.
pub fn classify(code: ErrorCode, message: &str) -> FailureKind {
    if REFUSALS.contains(&code) {
        return FailureKind::Refused;
    }
    if code == ErrorCode::RemoteConnectFailed
        && REFUSALS.iter().any(|refusal| {
            let lead = refusal.raw_string().split("{}").next().unwrap_or_default();
            !lead.is_empty() && message.starts_with(lead)
        })
    {
        return FailureKind::Refused;
    }
    FailureKind::Transient
}

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
///
/// `PreconnectCidNotRegistered` is the server saying it has no such account: after a
/// server that lost its accounts restarts, every attempt got it, and retrying it for
/// ten minutes kept the user on "Reconnecting" for an account that no longer exists.
const REFUSALS: [ErrorCode; 6] = [
    ErrorCode::AccountClientNonExists,
    ErrorCode::AccountServerNonExists,
    ErrorCode::AccountInvalidUsername,
    ErrorCode::AccountInvalidPassword,
    ErrorCode::AccountDisengaged,
    ErrorCode::PreconnectCidNotRegistered,
];

/// Whether a failed connect is worth repeating.
///
/// A refusal raised on this side keeps its code. One the SERVER sends arrives as
/// `RemoteConnectFailed` or `Generic` carrying the server's rendered error (a server
/// with no such account measured as `Generic`), so it is recognised by the text of the
/// same registry entry (see `renders`).
pub fn classify(code: ErrorCode, message: &str) -> FailureKind {
    if REFUSALS.contains(&code) {
        return FailureKind::Refused;
    }
    if matches!(code, ErrorCode::RemoteConnectFailed | ErrorCode::Generic)
        && REFUSALS
            .iter()
            .any(|refusal| renders(refusal.raw_string(), message))
    {
        return FailureKind::Refused;
    }
    FailureKind::Transient
}

/// Whether `message` is the registry form `form` rendered, at its start or right after
/// a `": "` (the server prefixes some errors with its own reason, e.g. "CID not
/// registered to this node: CID 7 is not registered to this node").
///
/// Every literal part of the form must appear, in order, with something in each
/// placeholder, and a form ending in literal text must end the message. A form that is
/// merely mentioned mid-sentence ("timed out after Invalid password") is not a match.
fn renders(form: &str, message: &str) -> bool {
    let parts: Vec<&str> = form.split("{}").collect();
    let (first, placeholders) = match parts.split_first() {
        Some((first, rest)) if !first.is_empty() => (*first, rest),
        _ => return false,
    };
    let after_separator = message.match_indices(": ").map(|(at, sep)| at + sep.len());
    std::iter::once(0).chain(after_separator).any(|start| {
        let Some(mut rest) = message[start..].strip_prefix(first) else {
            return false;
        };
        // Each later part follows a placeholder, which must hold something.
        for (index, part) in placeholders.iter().enumerate() {
            if index == placeholders.len() - 1 {
                return if part.is_empty() {
                    !rest.is_empty()
                } else {
                    rest.len() > part.len() && rest.ends_with(part)
                };
            }
            match rest.get(1..).and_then(|tail| tail.find(part)) {
                Some(at) => rest = &rest[1 + at + part.len()..],
                None => return false,
            }
        }
        true
    })
}

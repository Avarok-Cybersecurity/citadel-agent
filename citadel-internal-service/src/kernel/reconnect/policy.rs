//! When to try again, when to stop, and which failures end it at once. Pure: no clock,
//! no SDK, so every decision is tested directly (see policy_tests.rs).

use super::rendered::{renders, states};
use crate::kernel::reconnect::LinkState;
use citadel_internal_service_types::FailureReason;
use citadel_io::ErrorCode;
use citadel_proto::constants::{KEEP_ALIVE_INTERVAL_MS, KEEP_ALIVE_TIMEOUT_NS};
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
    /// How long after the drop the server may still hold the session that dropped, and
    /// so refuse every attempt as "already connected". While it says so, attempts go on
    /// this long instead of `give_up_after`. See `ServerHoldsSession`.
    pub server_holds_session_for: Duration,
}

/// The policy the agent runs. A deploy resets every socket at once and is back in
/// seconds; ten minutes covers a slow one without keeping a session up forever.
pub const SERVER_RECONNECT: ReconnectPolicy = ReconnectPolicy {
    first_delay: Duration::from_millis(500),
    max_delay: Duration::from_secs(30),
    give_up_after: Duration::from_secs(600),
    attempt_timeout: Duration::from_secs(30),
    server_holds_session_for: server_session_expiry(SDK_KEEP_ALIVE_TIMEOUT),
};

/// The keep-alive timeout a session gets when its Connect names none.
const SDK_KEEP_ALIVE_TIMEOUT: Duration = Duration::from_nanos(KEEP_ALIVE_TIMEOUT_NS as u64);

/// How often a server checks a session's keep-alives.
const SDK_KEEP_ALIVE_CHECK: Duration = Duration::from_millis(KEEP_ALIVE_INTERVAL_MS);

/// The latest a server ends a session whose keep-alives stopped: its checker runs once
/// per period and ends the session only once the last keep-alive is older than the
/// timeout, so up to one period past it. With the SDK's defaults that is an hour.
const fn server_session_expiry(keep_alive_timeout: Duration) -> Duration {
    keep_alive_timeout.saturating_add(SDK_KEEP_ALIVE_CHECK)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// The server will say the same thing next time: wrong password, no such account.
    Refused,
    /// The server is unreachable or not ready yet.
    Transient,
    /// The server still holds this account's session -- the one that dropped -- and
    /// refuses a second. It happens when only this side saw the link die: a reset that
    /// never reached the server leaves its end open, and it keeps the session until its
    /// keep-alive check ends it, up to an hour later. Every attempt until then is
    /// refused, and giving up after ten minutes removed a session the server was
    /// about to let go of: measured live, the account's chip vanished and nothing
    /// brought it back.
    ///
    /// The server replaces the agent's OWN dead session on the first reconnect (its resume
    /// token), so this means the held session is not provably this agent's: the token was
    /// lost with an agent restart, or another login replaced it.
    ServerHoldsSession,
}

/// How the SDK's server words a refusal of a second session for a CID it holds. The
/// SDK has no error code for it, so it is known by this lead (see `states`).
pub(crate) const SERVER_HOLDS_SESSION: &str = "Session Already Connected";

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

/// What the UI is told about a reconnect the server refused, so it can act on it. A refused
/// sign-in is named only here: on a Connect the same code is a mistyped password, which the
/// sign-in form already words itself.
pub fn refusal_reason(refused: ErrorCode) -> Option<FailureReason> {
    match refused {
        ErrorCode::PqSignInFailed => Some(FailureReason::SignInRefused),
        ErrorCode::ResumeCredentialExpired => Some(FailureReason::ResumeCredentialExpired),
        ErrorCode::ResumeCredentialRevoked => Some(FailureReason::ResumeCredentialRevoked),
        other => crate::kernel::sign_in::failure_reason(other),
    }
}

/// What the UI is told a reconnect that ran out of time was waiting on. A server that never
/// once answered is unreachable; one that answered "still connected" throughout is not.
pub fn out_of_time_reason(last: FailureKind) -> Option<FailureReason> {
    match last {
        FailureKind::Transient => Some(FailureReason::ServerUnreachable),
        FailureKind::Refused | FailureKind::ServerHoldsSession => None,
    }
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
        // During a takeover the drops reported are the stopped reconnect's own attempts.
        LinkState::Reconnecting | LinkState::SigningIn => DropAction::AlreadyReconnecting,
        LinkState::Ending => DropAction::Remove,
    }
}

/// Whether a claim must find the session in the SDK before handing it over.
///
/// Not while the agent is reconnecting it: the SDK holds no session for it until the
/// reconnect lands, by design (connect.rs already knows this). The claim read that
/// absence as death, removed the session and so stopped its reconnect -- a page
/// reloaded during a server drop signed its user out.
pub fn claim_requires_sdk_session(link: LinkState) -> bool {
    !matches!(link, LinkState::Reconnecting | LinkState::SigningIn)
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
        if elapsed.saturating_add(delay) > self.limit(kind) {
            return Next::GiveUp(GiveUp::OutOfTime);
        }
        Next::RetryAfter(delay)
    }

    /// How long after the drop attempts may still start, given how the last one failed.
    pub fn limit(&self, kind: FailureKind) -> Duration {
        match kind {
            FailureKind::ServerHoldsSession => {
                self.give_up_after.max(self.server_holds_session_for)
            }
            FailureKind::Refused | FailureKind::Transient => self.give_up_after,
        }
    }

    /// This policy for a session whose Connect named `keep_alive_timeout`: the server
    /// holds a dead session by that session's own keep-alive, not the default one.
    pub fn for_keep_alive(self, keep_alive_timeout: Option<Duration>) -> Self {
        let server_holds_session_for = match keep_alive_timeout {
            None => return self,
            // Zero turns keep-alives off, so the server never ends a dead session on
            // its own: there is no expiry to wait for, and the usual limit stands.
            Some(timeout) if timeout.as_secs() == 0 => Duration::ZERO,
            // The SDK sends whole seconds.
            Some(timeout) => server_session_expiry(Duration::from_secs(timeout.as_secs())),
        };
        Self {
            server_holds_session_for,
            ..self
        }
    }
}

/// The account errors a retry cannot fix. From the SDK's registry, not copied strings.
///
/// `PreconnectRefused` is the server not going on with the connect's first packet: an account
/// it does not hold (a server that restarted without its accounts) or a packet that does not
/// open under the account's keys. Asking again with the same keys gets the same answer.
///
/// The admission refusals are the server's Turnstile check: a reconnect cannot show the
/// widget, so retrying asked the same question every attempt for ten minutes. The user has
/// to sign in again (the failure says so, see `refusal`).
///
/// `PqSignInFailed` is the server's one answer to a post-quantum sign-in that did not prove
/// the account ("Authentication failed"): the password this session kept is no longer the
/// account's, or the account now asks for a factor a reconnect cannot give. It was retried
/// for the whole ten minutes, 24 attempts, each refused the same way.
const REFUSALS: [ErrorCode; 12] = [
    ErrorCode::AccountClientNonExists,
    ErrorCode::AccountServerNonExists,
    ErrorCode::AccountInvalidUsername,
    ErrorCode::AccountInvalidPassword,
    ErrorCode::AccountDisengaged,
    ErrorCode::PreconnectRefused,
    ErrorCode::PqSignInAdmissionRequired,
    ErrorCode::PqSignInAdmissionFailed,
    ErrorCode::PqSignInFailed,
    // The session's resume credential ran out, or predates a change to the account's
    // sign-in: every further attempt would be refused the same way.
    ErrorCode::ResumeCredentialExpired,
    ErrorCode::ResumeCredentialRevoked,
    // One side must be updated; no number of attempts changes that.
    ErrorCode::ProtocolVersionUnsupported,
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
    if states(message, SERVER_HOLDS_SESSION) {
        return FailureKind::ServerHoldsSession;
    }
    if refusal(code, message).is_some() {
        return FailureKind::Refused;
    }
    FailureKind::Transient
}

/// The registry entry a failed connect is a refusal of, by its code or, for one the server
/// sent, by the text it rendered (see `classify`).
pub fn refusal(code: ErrorCode, message: &str) -> Option<ErrorCode> {
    if REFUSALS.contains(&code) {
        return Some(code);
    }
    if !matches!(code, ErrorCode::RemoteConnectFailed | ErrorCode::Generic) {
        return None;
    }
    REFUSALS
        .iter()
        .copied()
        .find(|refusal| renders(refusal.raw_string(), message))
}

//! Resume credentials, as the agent reports them: how a session's row is marked, and the
//! moment a session's credential runs out.
//!
//! After a full sign-in the agent may hold a resume credential for the session: what lets a
//! dropped link come back with no password and no security-key touch, until
//! `valid_until_utc`. Past that moment, or once the account's sign-in has changed, the next
//! drop ends in a sign-in.

use crate::FailureReason;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// Where a session stands, for whoever lists it.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum SessionRowStatus {
    /// Its link is up and it can come back from a drop by itself.
    Live,
    /// Its link is down and the agent is bringing it back.
    Reconnecting,
    /// Its resume credential has expired or been refused: its next drop, or the one that
    /// already happened, ends in a sign-in.
    NeedsSignIn,
}

/// The dot beside a session's row. Every client draws the dot the agent names; none decides
/// a colour from a status itself.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum SessionDot {
    Green,
    Yellow,
}

impl SessionRowStatus {
    /// A session in the agent's map: `link_up` is its link's state, `resume_ended` whether a
    /// resume credential it held has expired.
    pub fn of_session(link_up: bool, resume_ended: bool) -> Self {
        match (resume_ended, link_up) {
            (true, _) => Self::NeedsSignIn,
            (false, true) => Self::Live,
            (false, false) => Self::Reconnecting,
        }
    }

    /// A session the agent gave up on, by why: one whose resume credential expired or was
    /// refused needs its sign-in, and is marked. Any other give-up is not a status of this
    /// kind, and its row is drawn as it always was.
    pub fn of_signed_out(reason: Option<FailureReason>) -> Option<Self> {
        match reason {
            Some(
                FailureReason::ResumeCredentialExpired | FailureReason::ResumeCredentialRevoked,
            ) => Some(Self::NeedsSignIn),
            _ => None,
        }
    }

    /// The one mapping from a status to its dot.
    pub fn dot(self) -> SessionDot {
        match self {
            Self::Live | Self::Reconnecting => SessionDot::Green,
            Self::NeedsSignIn => SessionDot::Yellow,
        }
    }
}

/// A session's resume credential ran out at `valid_until_utc`. Sent once, when it happens,
/// whether or not the link is up; the session itself goes on until its link next drops.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ResumeCredentialExpired {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    /// Unix milliseconds, UTC, by the server's clock.
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub valid_until_utc: u64,
    pub request_id: Option<Uuid>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Control: answer `Green` for `NeedsSignIn`.
    #[test]
    fn a_session_that_needs_its_sign_in_is_yellow_and_every_other_is_green() {
        assert_eq!(SessionRowStatus::Live.dot(), SessionDot::Green);
        assert_eq!(SessionRowStatus::Reconnecting.dot(), SessionDot::Green);
        assert_eq!(SessionRowStatus::NeedsSignIn.dot(), SessionDot::Yellow);
    }

    /// Control: read `resume_ended` after `link_up`.
    #[test]
    fn an_ended_credential_marks_the_session_whatever_its_link() {
        assert_eq!(
            SessionRowStatus::of_session(true, false),
            SessionRowStatus::Live
        );
        assert_eq!(
            SessionRowStatus::of_session(false, false),
            SessionRowStatus::Reconnecting
        );
        for link_up in [true, false] {
            assert_eq!(
                SessionRowStatus::of_session(link_up, true),
                SessionRowStatus::NeedsSignIn
            );
        }
    }

    /// Control: mark every signed-out session.
    #[test]
    fn only_a_give_up_over_the_resume_credential_marks_a_signed_out_session() {
        use FailureReason::*;
        for reason in [ResumeCredentialExpired, ResumeCredentialRevoked] {
            let status = SessionRowStatus::of_signed_out(Some(reason));
            assert_eq!(status.map(SessionRowStatus::dot), Some(SessionDot::Yellow));
        }
        for reason in [
            None,
            Some(SignInRefused),
            Some(ServerUnreachable),
            Some(Unknown),
        ] {
            assert_eq!(SessionRowStatus::of_signed_out(reason), None, "{reason:?}");
        }
    }

    #[test]
    fn the_wire_names_are_the_ones_clients_read() {
        let json = |value| serde_json::to_string(&value).unwrap();
        assert_eq!(json(SessionDot::Yellow), "\"yellow\"");
        assert_eq!(json(SessionDot::Green), "\"green\"");
        let status = serde_json::to_string(&SessionRowStatus::NeedsSignIn).unwrap();
        assert_eq!(status, "\"needs_sign_in\"");
    }
}

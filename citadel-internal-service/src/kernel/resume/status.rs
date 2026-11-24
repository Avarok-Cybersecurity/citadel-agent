//! A session's row, from what the agent holds for it. The colours are the wire types'
//! (`SessionRowStatus::dot`); this only reads the session.

use super::wall_clock_ms;
use crate::kernel::reconnect::{LinkState, Reauth};
use citadel_internal_service_types::SessionRowStatus;

/// What `GetSessions` says of a session's standing.
pub(crate) struct Standing {
    pub status: SessionRowStatus,
    /// Until when its resume credential lasts (Unix milliseconds, UTC, the server's clock).
    pub resume_valid_until_utc: Option<u64>,
}

/// The standing of a session whose link is `link` and which signs in again with `reauth`,
/// when this computer's clock reads `now_ms`.
pub(crate) fn standing_at(link: LinkState, reauth: &Reauth, now_ms: u64) -> Standing {
    let held = match reauth {
        Reauth::Resume(credential) => Some(credential),
        Reauth::Password(_) | Reauth::NeedsUser(_) => None,
    };
    let ended = held.is_some_and(|credential| credential.expired_at(now_ms));
    Standing {
        status: SessionRowStatus::of_session(link == LinkState::Up, ended),
        resume_valid_until_utc: held.map(|credential| credential.valid_until_utc()),
    }
}

pub(crate) fn standing(link: LinkState, reauth: &Reauth) -> Standing {
    standing_at(link, reauth, wall_clock_ms())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::resume::test_support::{credential, ISSUED_AT};
    use citadel_internal_service_types::SessionDot;
    use citadel_sdk::prelude::SecBuffer;

    const HOUR_MS: u64 = 3_600_000;

    /// Control: read the row's status from the link alone.
    #[tokio::test]
    async fn a_session_is_marked_from_the_moment_its_credential_expires_link_up_or_not() {
        let held = Reauth::Resume(credential(3_600).await);
        let at = |link, now| standing_at(link, &held, now);
        let before = at(LinkState::Up, ISSUED_AT + HOUR_MS - 1);
        assert_eq!(before.status, SessionRowStatus::Live);
        assert_eq!(before.status.dot(), SessionDot::Green);
        assert_eq!(before.resume_valid_until_utc, Some(ISSUED_AT + HOUR_MS));
        for link in [LinkState::Up, LinkState::Reconnecting] {
            let after = at(link, ISSUED_AT + HOUR_MS);
            assert_eq!(after.status, SessionRowStatus::NeedsSignIn, "{link:?}");
            assert_eq!(after.status.dot(), SessionDot::Yellow);
        }
        let dropped = at(LinkState::Reconnecting, ISSUED_AT);
        assert_eq!(dropped.status, SessionRowStatus::Reconnecting);
    }

    #[test]
    fn a_session_with_no_credential_has_none_to_expire() {
        for reauth in [
            Reauth::Password(SecBuffer::from("pw")),
            Reauth::NeedsUser("sign in again"),
        ] {
            let standing = standing_at(LinkState::Up, &reauth, u64::MAX);
            assert_eq!(standing.status, SessionRowStatus::Live);
            assert_eq!(standing.resume_valid_until_utc, None);
        }
    }
}

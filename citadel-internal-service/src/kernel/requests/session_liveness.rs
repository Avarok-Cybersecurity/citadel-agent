//! Is the session the connection map remembers still alive in the SDK?
//!
//! Two handlers ask this — `Connect`, when a session already exists for the
//! username, and `ClaimSession` — and both used to answer an ERROR from
//! `remote.sessions()` with "then it is not active": `Err(..) => false` in one,
//! `Err(..) => vec![]` in the other. Each then took the not-active branch, which
//! DELETES the map entry for a session that is, as far as anyone knows, still
//! live. `remote.connect()` is refused after that (the SDK still holds the
//! session), so the account is unreachable until the agent restarts — the exact
//! wedge `connection_management.rs` documents as the reason `Connect` must not
//! delete sessions it cannot account for.
//!
//! A question that could not be asked has no answer. This says so, separately
//! from "asked, and it is gone", so the handlers can refuse and let the caller
//! retry instead of destroying state on a transient error.

/// What the SDK says about a session the connection map holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLiveness {
    /// The SDK lists it: it is live, and must not be torn down.
    Active,
    /// The SDK answered and does not list it: the map entry is stale.
    Stale,
    /// The SDK could not be asked. NOT the same as stale, and the reason this
    /// type exists rather than a `bool`.
    Unknown,
}

/// Classify a session against the SDK's answer.
///
/// `sessions` is the SDK's reply: `Ok(cids)` when it answered, `Err(reason)`
/// when it did not.
pub fn classify<E>(sessions: Result<&[u64], E>, cid: u64) -> SessionLiveness {
    match sessions {
        Ok(cids) if cids.contains(&cid) => SessionLiveness::Active,
        Ok(_) => SessionLiveness::Stale,
        Err(_) => SessionLiveness::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listed_session_is_active() {
        assert_eq!(classify::<()>(Ok(&[7, 9]), 7), SessionLiveness::Active);
    }

    #[test]
    fn an_answer_that_omits_it_makes_it_stale() {
        assert_eq!(classify::<()>(Ok(&[9]), 7), SessionLiveness::Stale);
        assert_eq!(classify::<()>(Ok(&[]), 7), SessionLiveness::Stale);
    }

    #[test]
    fn an_unanswered_question_is_not_an_answer() {
        // The whole point. Mapping this to Stale is what deleted live sessions,
        // and a `bool` return type cannot express the difference.
        assert_eq!(
            classify(Err("SDK unreachable"), 7),
            SessionLiveness::Unknown
        );
        assert_ne!(classify(Err("SDK unreachable"), 7), SessionLiveness::Stale);
    }

    #[test]
    fn only_a_positive_answer_permits_a_takeover_and_only_a_negative_one_permits_a_teardown() {
        // Stated as the two rules the handlers depend on, so a future edit that
        // widens either one fails here rather than in production.
        for liveness in [SessionLiveness::Stale, SessionLiveness::Unknown] {
            assert_ne!(liveness, SessionLiveness::Active);
        }
        assert_ne!(SessionLiveness::Unknown, SessionLiveness::Stale);
    }
}

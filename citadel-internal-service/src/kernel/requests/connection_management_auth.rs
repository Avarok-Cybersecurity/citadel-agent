//! Who may act on a session through `ConnectionManagement`.
//!
//! `ConnectionManagement` is one of the six variants `session_cid()` returns
//! `None` for, so the ownership gate in `requests/mod.rs` never sees it. That
//! was deliberate — the variant carries its target inside `management_command`
//! rather than in a `cid` field — but the consequence was that every command in
//! it acted on whatever CID the caller named, with nothing checked:
//!
//! * `ClaimSession { only_if_orphaned: false }` re-pointed a LIVE session's
//!   message stream to the caller. Every subsequent notification for that
//!   session — messages, file-transfer ticks, call media — went to the thief,
//!   and the owner was locked out with no signal that anything had happened.
//! * `DisconnectOrphan { session_cid: Some(cid) }` removed any session from the
//!   map, orphaned or not, without ever consulting whether it was orphaned.
//! * `ReleaseSession` re-pointed any session's owner to the nil UUID, marking
//!   somebody else's live session as reclaimable.
//!
//! A CID is a `u64` that travels in peer lists and notifications; it is not a
//! secret. `GetSessions` will hand over every one of them. So "names a CID" was
//! the whole of the authorization.
//!
//! The rule here is the one the code already believed it had: you may act on a
//! session that is **orphaned** (nobody holds it) or **already yours**. A live
//! session held by a different localhost connection is refused.
//!
//! That is not a weakening of hand-off. An authenticated hand-off between two
//! live connections already exists and is the correct door: `Connect` proves
//! knowledge of the password via `credential_fingerprint` and then re-points
//! the session itself. This module refuses the unauthenticated shortcut around
//! it — the same fix, propagated to the sibling door it was never applied to.

use uuid::Uuid;

/// What the connection map says about a session, reduced to what authorization
/// needs to decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOwner {
    /// The owning localhost connection is still connected.
    Live(Uuid),
    /// The owning connection is gone. The session is reclaimable by anyone —
    /// this is the case the reload-and-reclaim flow depends on.
    Orphaned,
}

/// The decision. `Refuse` carries the message sent back to the caller.
#[derive(Debug, PartialEq, Eq)]
pub enum Authorization {
    Allow,
    Refuse(String),
}

impl Authorization {
    #[cfg(test)]
    fn is_allowed(&self) -> bool {
        matches!(self, Authorization::Allow)
    }
}

/// The single rule, shared by all three commands.
///
/// Kept as one function rather than three near-copies on purpose: three copies
/// of a check is how two of them come to differ, and that is the exact defect
/// this module exists to close.
fn owner_or_orphan(owner: SessionOwner, caller: Uuid, session_cid: u64) -> Authorization {
    match owner {
        SessionOwner::Orphaned => Authorization::Allow,
        SessionOwner::Live(held_by) if held_by == caller => Authorization::Allow,
        SessionOwner::Live(_) => Authorization::Refuse(format!(
            "Session {session_cid} is in use by another connection"
        )),
    }
}

/// May `caller` re-point this session's message stream to itself?
pub fn may_claim(owner: SessionOwner, caller: Uuid, session_cid: u64) -> Authorization {
    owner_or_orphan(owner, caller, session_cid)
}

/// May `caller` mark this session orphaned?
pub fn may_release(owner: SessionOwner, caller: Uuid, session_cid: u64) -> Authorization {
    owner_or_orphan(owner, caller, session_cid)
}

/// May `caller` remove this session from the connection map?
pub fn may_disconnect(owner: SessionOwner, caller: Uuid, session_cid: u64) -> Authorization {
    owner_or_orphan(owner, caller, session_cid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller() -> Uuid {
        Uuid::from_u128(1)
    }

    fn stranger() -> Uuid {
        Uuid::from_u128(2)
    }

    /// The reload-and-reclaim flow. Every product call site passes
    /// `only_if_orphaned: true` and lands here.
    #[test]
    fn an_orphaned_session_may_be_claimed_by_anyone() {
        assert!(may_claim(SessionOwner::Orphaned, caller(), 7).is_allowed());
    }

    /// `peer-registration-store/lifecycle.ts` claims its own live session
    /// before sending PeerRegister. That must keep working, and it is the
    /// reason this is not simply "orphaned only".
    #[test]
    fn a_connection_may_reassert_a_session_it_already_holds() {
        assert!(may_claim(SessionOwner::Live(caller()), caller(), 7).is_allowed());
    }

    /// C1. The whole point.
    #[test]
    fn a_live_session_held_elsewhere_may_not_be_claimed() {
        let decision = may_claim(SessionOwner::Live(stranger()), caller(), 7);
        assert_eq!(
            decision,
            Authorization::Refuse("Session 7 is in use by another connection".to_string())
        );
    }

    /// The refusal must not collide with the "is not orphaned" string, which
    /// `claim-session.ts` matches on to distinguish "another tab has it" from a
    /// real error. A refusal that read "not orphaned" would be swallowed by
    /// that branch and reported to the user as success.
    #[test]
    fn the_refusal_is_distinguishable_from_the_not_orphaned_message() {
        let Authorization::Refuse(message) = may_claim(SessionOwner::Live(stranger()), caller(), 7)
        else {
            panic!("expected a refusal");
        };
        assert!(!message.contains("not orphaned"));
    }

    /// H1, first half.
    #[test]
    fn a_live_session_held_elsewhere_may_not_be_disconnected() {
        assert!(!may_disconnect(SessionOwner::Live(stranger()), caller(), 7).is_allowed());
    }

    /// H1, second half. `ReleaseSession` means "this tab is done with it", so
    /// releasing a session another tab is using is never legitimate.
    #[test]
    fn a_live_session_held_elsewhere_may_not_be_released() {
        assert!(!may_release(SessionOwner::Live(stranger()), caller(), 7).is_allowed());
    }

    /// Teardown releases the sessions this connection owns.
    #[test]
    fn a_connection_may_release_its_own_session() {
        assert!(may_release(SessionOwner::Live(caller()), caller(), 7).is_allowed());
    }

    /// The bulk `DisconnectOrphan { session_cid: None }` branch already filters
    /// to orphans; the targeted branch did not. Pin that an orphan stays
    /// removable so that fix does not regress into "owner only".
    #[test]
    fn an_orphan_may_still_be_disconnected() {
        assert!(may_disconnect(SessionOwner::Orphaned, caller(), 7).is_allowed());
    }

    /// The nil UUID is what `ReleaseSession` stamps as the orphan marker. If a
    /// caller's own connection id were ever nil, `Live(nil)` would compare
    /// equal to it and a released session would read as "already yours".
    /// Connection ids come from `Uuid::new_v4()`, which never produces nil, but
    /// the released state is represented as `Orphaned` here rather than as
    /// `Live(nil)` precisely so that coincidence cannot arise.
    #[test]
    fn the_orphan_marker_is_never_mistaken_for_a_holder() {
        assert!(!may_claim(SessionOwner::Live(Uuid::nil()), caller(), 7).is_allowed());
    }
}

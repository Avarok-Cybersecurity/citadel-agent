//! What a user's sign-in does to a session the agent already tracks for the account. Pure:
//! the SDK query, the fingerprint and the handoff are the caller's (requests/connect.rs,
//! reconnect/takeover.rs).
//!
//! A session the agent is reconnecting used to answer `SessionAlreadyActive` to a sign-in,
//! so a user whose server still held their dead session was sent to the navbar for up to an
//! hour. Since Citadel-Protocol #319 a forced login that authenticates replaces what the
//! server holds, so a sign-in that proves the password takes the reconnect over instead.

use super::LinkState;

/// What the agent knows about the tracked session when the sign-in arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tracked {
    /// The SDK holds it: it is live.
    Live,
    /// The agent is bringing it back after its server dropped it; the SDK holds nothing.
    Reconnecting,
    /// The agent still lists it but the SDK does not, and nothing is reconnecting it.
    Stale,
}

/// Classify the tracked session from its link state and whether the SDK holds it.
pub fn tracked(link: LinkState, sdk_holds: bool) -> Tracked {
    match (link, sdk_holds) {
        (LinkState::Reconnecting | LinkState::SigningIn, _) => Tracked::Reconnecting,
        (_, true) => Tracked::Live,
        (_, false) => Tracked::Stale,
    }
}

/// What to do with the sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignIn {
    /// Wrong password for a tracked session: refused exactly as a wrong password on an
    /// untracked account is, and nothing about the session changes.
    Refuse,
    /// Live: hand the caller the session as it is (unchanged behaviour).
    AlreadyActive,
    /// Stop the reconnect, then connect with `force_login` into the same entry.
    TakeOverReconnect,
    /// Drop the stale entry and connect afresh; the server checks the password.
    ReplaceStale,
}

/// `authorized`: the presented password matches the one the session was opened with. It
/// is not consulted for a stale entry, whose connect the server authenticates itself.
pub fn on_sign_in(tracked: Tracked, authorized: bool) -> SignIn {
    match (tracked, authorized) {
        (Tracked::Stale, _) => SignIn::ReplaceStale,
        (Tracked::Live | Tracked::Reconnecting, false) => SignIn::Refuse,
        (Tracked::Live, true) => SignIn::AlreadyActive,
        (Tracked::Reconnecting, true) => SignIn::TakeOverReconnect,
    }
}

/// Whether a sign-in may start taking over a session in `link`. Checked and changed under
/// one lock, so two sign-ins, or a sign-in and a Disconnect, cannot both proceed.
pub fn may_take_over(link: LinkState) -> bool {
    link == LinkState::Reconnecting
}

/// Whether reconnect run `run` may start another attempt: the session is still waiting
/// for it (not ended, not being signed in to) and no newer run has replaced it.
pub fn attempt_wanted(link: LinkState, current_run: u64, run: u64) -> bool {
    link == LinkState::Reconnecting && current_run == run
}

/// Whether a link run `run` just opened goes into the entry. A sign-in that is waiting
/// on this attempt (`SigningIn`) wants it; an ended session does not.
pub fn attempt_installs(link: LinkState, current_run: u64, run: u64) -> bool {
    matches!(link, LinkState::Reconnecting | LinkState::SigningIn) && current_run == run
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reconnecting_session_is_taken_over_only_with_the_right_password() {
        let reconnecting = tracked(LinkState::Reconnecting, false);
        assert_eq!(reconnecting, Tracked::Reconnecting);
        assert_eq!(on_sign_in(reconnecting, true), SignIn::TakeOverReconnect);
        assert_eq!(
            on_sign_in(reconnecting, false),
            SignIn::Refuse,
            "a wrong password leaves the reconnect alone"
        );
    }

    #[test]
    fn a_live_session_is_still_answered_already_active() {
        let live = tracked(LinkState::Up, true);
        assert_eq!(live, Tracked::Live);
        assert_eq!(on_sign_in(live, true), SignIn::AlreadyActive);
        assert_eq!(on_sign_in(live, false), SignIn::Refuse);
    }

    #[test]
    fn a_stale_entry_is_replaced_and_the_server_judges_the_password() {
        let stale = tracked(LinkState::Up, false);
        assert_eq!(stale, Tracked::Stale);
        assert_eq!(on_sign_in(stale, false), SignIn::ReplaceStale);
        assert_eq!(on_sign_in(stale, true), SignIn::ReplaceStale);
    }

    #[test]
    fn a_takeover_stops_the_reconnect_but_adopts_its_attempt_in_flight() {
        assert!(attempt_wanted(LinkState::Reconnecting, 3, 3));
        assert!(
            !attempt_wanted(LinkState::SigningIn, 3, 3),
            "no new attempt once a sign-in is taking over"
        );
        assert!(
            attempt_installs(LinkState::SigningIn, 3, 3),
            "the attempt the sign-in waited on is the session it wants"
        );
        for link in [LinkState::Up, LinkState::Ending] {
            assert!(!attempt_wanted(link, 3, 3) && !attempt_installs(link, 3, 3));
        }
    }

    #[test]
    fn an_older_run_neither_attempts_nor_installs() {
        assert!(!attempt_wanted(LinkState::Reconnecting, 4, 3));
        assert!(!attempt_installs(LinkState::Reconnecting, 4, 3));
        assert!(!attempt_installs(LinkState::SigningIn, 4, 3));
    }

    #[test]
    fn a_session_already_being_taken_over_is_not_taken_over_twice() {
        assert!(may_take_over(LinkState::Reconnecting));
        for link in [LinkState::Up, LinkState::Ending, LinkState::SigningIn] {
            assert!(!may_take_over(link), "{link:?}");
        }
        assert_eq!(
            tracked(LinkState::SigningIn, false),
            Tracked::Reconnecting,
            "a second sign-in during a takeover is judged like one during the reconnect"
        );
    }
}

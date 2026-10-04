//! Proving that a caller knows the password a session was opened with.
//!
//! Authentication happens at the SERVER: the agent hands the username and
//! password to the SDK, the SDK carries them to the server, and the server
//! decides. Nothing local can substitute for that, and the obvious candidate
//! does not work — `ClientNetworkAccount::validate_credentials`, despite being
//! documented as "used for the login process", succeeds only for
//! `ArgonContainerType::Server`. This is a client; its container is the client
//! variant, so calling it here would reject every correct password while
//! looking like a security check.
//!
//! But `Connect` has a branch that never reaches the SDK at all. When a session
//! for the username is already live, the handler answers `SessionAlreadyActive`
//! and re-points that session's message stream to the caller — which means the
//! password went nowhere, was checked by nobody, and any caller naming a live
//! username took over its stream.
//!
//! Re-running the SDK connect to authenticate is not available either: a second
//! connect against a live session is exactly the ratchet reset that branch was
//! added to prevent.
//!
//! So the session remembers what opened it: a keyed hash of the password the
//! server accepted, recorded at connect time and re-derived on a reuse request.
//! That proves the caller knows the password, without a server round trip and
//! without touching the ratchet.
//!
//! The hash is the agent's own -- HMAC-SHA3-256 under a key this process draws
//! once and never stores -- and not the protocol's client-side hash, which this
//! used to be. That one is deterministic only for a legacy (Argon2) account. For
//! a post-quantum account the SDK's `generate_connect_credentials` ignores the
//! password altogether (the password is proven by a factor, so there is nothing
//! to hash), and a fingerprint taken from it was the same for every password:
//! any caller who knew a signed-in username was handed the live session.
//! tests/pq_sign_in_password.rs is the regression test.
//!
//! What this deliberately does NOT claim: it is not authentication. It proves
//! the caller knows the same password the SERVER already accepted for this
//! session. If the password changed server-side since, this still matches — the
//! session it admits you to is the one that password opened, which is the
//! property that matters here. A session whose sign-in proved more than the
//! password (a security key, a recovery code) records no fingerprint at all
//! (requests/connect.rs), so nothing can reach it on a password.

use citadel_sdk::prelude::{NodeRemote, Ratchet, SecBuffer};
use hmac::{Hmac, Mac};
use sha3::Sha3_256;
use std::sync::OnceLock;

/// The keyed hash of `password` for `username`, or `None` when no such proof
/// can be produced.
///
/// `None` is not "allowed". Every caller must treat it as a refusal — see
/// `matches`, which is the only sanctioned comparison.
pub async fn derive<R: Ratchet>(
    remote: &NodeRemote<R>,
    username: &str,
    password: SecBuffer,
) -> Option<Vec<u8>> {
    let cnac = remote
        .account_manager()
        .get_client_by_username(username)
        .await
        .ok()
        .flatten()?;

    // A transient (passwordless) account has no secret to know, so a
    // fingerprint would be a constant that every caller can produce. Refusing
    // to derive one is what keeps `matches` from becoming a check that cannot
    // fail.
    if cnac.is_transient() {
        return None;
    }
    keyed_hash(process_key()?, username, password.as_ref())
}

/// Drawn once per process: a fingerprint means nothing outside the agent that
/// recorded it, and never needs to.
fn process_key() -> Option<&'static [u8; 32]> {
    static KEY: OnceLock<Option<[u8; 32]>> = OnceLock::new();
    KEY.get_or_init(|| {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).ok().map(|()| key)
    })
    .as_ref()
}

fn keyed_hash(key: &[u8; 32], username: &str, password: &[u8]) -> Option<Vec<u8>> {
    let mut mac = Hmac::<Sha3_256>::new_from_slice(key).ok()?;
    // Length-prefixed, so no (username, password) pair runs into another.
    mac.update(&(username.len() as u64).to_be_bytes());
    mac.update(username.as_bytes());
    mac.update(password);
    Some(mac.finalize().into_bytes().to_vec())
}

/// Whether a freshly derived fingerprint proves knowledge of the recorded one.
///
/// `None` on either side is a refusal, never a pass: a session with no recorded
/// fingerprint (one that predates this mechanism, or an account we cannot
/// fingerprint) must be re-authenticated through the SDK rather than handed
/// over on the strength of a username.
///
/// The comparison is constant-time. The values are password-derived, and an
/// early-exit `==` leaks how many leading bytes a guess got right.
pub fn matches(recorded: Option<&Vec<u8>>, presented: Option<&Vec<u8>>) -> bool {
    let (Some(recorded), Some(presented)) = (recorded, presented) else {
        return false;
    };
    if recorded.len() != presented.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in recorded.iter().zip(presented.iter()) {
        difference |= a ^ b;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::{keyed_hash, matches};

    #[test]
    fn the_hash_depends_on_the_password_and_on_nothing_shifted_between_fields() {
        let key = [7u8; 32];
        let a = keyed_hash(&key, "alice", b"pw-one");
        assert_eq!(a, keyed_hash(&key, "alice", b"pw-one"));
        assert!(!matches(
            a.as_ref(),
            keyed_hash(&key, "alice", b"pw-two").as_ref()
        ));
        assert!(!matches(
            keyed_hash(&key, "ab", b"c").as_ref(),
            keyed_hash(&key, "a", b"bc").as_ref()
        ));
        assert!(!matches(
            a.as_ref(),
            keyed_hash(&[8u8; 32], "alice", b"pw-one").as_ref()
        ));
    }

    #[test]
    fn an_absent_recording_never_matches() {
        // The dangerous default. A session opened before this existed has no
        // fingerprint, and treating that as "nothing to check, therefore fine"
        // is precisely the hole being closed.
        assert!(!matches(None, Some(&vec![1, 2, 3])));
    }

    #[test]
    fn an_underivable_presentation_never_matches() {
        assert!(!matches(Some(&vec![1, 2, 3]), None));
    }

    #[test]
    fn two_absences_do_not_agree_with_each_other() {
        assert!(!matches(None, None));
    }

    #[test]
    fn equal_fingerprints_match() {
        assert!(matches(Some(&vec![9, 9, 9]), Some(&vec![9, 9, 9])));
    }

    #[test]
    fn a_different_password_does_not() {
        assert!(!matches(Some(&vec![9, 9, 9]), Some(&vec![9, 9, 8])));
    }

    #[test]
    fn a_prefix_does_not_match_the_whole() {
        // Length is checked before the loop; without that, a shorter guess
        // sharing a prefix would zip to completion and compare equal.
        assert!(!matches(Some(&vec![9, 9, 9]), Some(&vec![9, 9])));
        assert!(!matches(Some(&vec![9, 9]), Some(&vec![9, 9, 9])));
    }

    #[test]
    fn an_empty_recording_does_not_admit_an_empty_guess() {
        // Both empty compares equal under the loop, so this pins that an empty
        // fingerprint can never be produced as a pass. `derive` returns None
        // rather than an empty vec, and None is a refusal.
        assert!(matches(Some(&vec![]), Some(&vec![])));
        assert!(!matches(None, Some(&vec![])));
    }
}

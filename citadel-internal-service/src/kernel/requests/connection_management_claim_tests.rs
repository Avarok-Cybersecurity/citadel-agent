use super::*;

fn caller() -> Uuid {
    Uuid::from_u128(10)
}

fn rival() -> Uuid {
    Uuid::from_u128(20)
}

/// M1's losing side. This connection saw the session orphaned before its
/// SDK await; by the time it holds the write lock, the rival's claim has
/// landed. Fresh state must refuse, with the message the UI already
/// handles as "another tab has it" — the same answer a serialized
/// ordering would have given.
#[test]
fn a_claim_that_lost_the_race_is_refused_on_fresh_state() {
    let decision = decide_claim(SessionOwner::Live(rival()), true, caller(), 7);
    assert_eq!(decision, Err("Session 7 is not orphaned".to_string()));
}

/// Same race under `only_if_orphaned: false`: the flag authorizes
/// nothing, and the message is the one session_takeover.rs pins.
#[test]
fn a_forced_claim_that_lost_the_race_is_refused() {
    let decision = decide_claim(SessionOwner::Live(rival()), false, caller(), 7);
    assert_eq!(
        decision,
        Err("Session 7 is in use by another connection".to_string())
    );
}

/// The winner re-checks too; a session still orphaned at the write lock
/// must pass, or no claim would ever succeed.
#[test]
fn a_still_orphaned_session_passes_the_recheck() {
    assert_eq!(
        decide_claim(SessionOwner::Orphaned, true, caller(), 7),
        Ok(())
    );
}

/// A connection reasserting a session it already holds (the
/// peer-registration-store flow) must survive the recheck as well.
#[test]
fn reasserting_an_owned_session_passes() {
    assert_eq!(
        decide_claim(SessionOwner::Live(caller()), false, caller(), 7),
        Ok(())
    );
}

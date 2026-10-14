//! A peer that never answers is dialled less and less often, and promptly again once there
//! is word it is back. The live case: one peer, dialled every ~85 s for 25 minutes because
//! the 30 s dial backoff is short beside a 60 s dial that fails.

use super::super::types::{DialOutcome, Input};
use super::tests::{core, secs, tick, PEER};
use super::tests_peers::{dials, failed, lost, up, QUIET};
use super::Core;
use citadel_internal_service_types::P2pPathReport;

/// A queued message makes the peer wanted, and the first dial goes out at once.
fn wanted_and_dialling() -> Core {
    let mut core = core(QUIET);
    let told = core.step(Input::Backlog {
        peer: PEER,
        pending: 3,
    });
    assert_eq!(dials(&told), 1, "{told:?}");
    core
}

/// Seconds until the next dial is due, after a dial that failed at `at`.
fn wait_after_failure(core: &mut Core, at: u64) -> u64 {
    failed(core, at);
    let due = core.next_wake().expect("a wanted peer is dialled again");
    due.since(secs(at)).as_secs()
}

/// Fails the dial at `at`, then runs the next one when it is due; returns that time.
fn fail_and_redial(core: &mut Core, at: u64) -> (u64, u64) {
    let wait = wait_after_failure(core, at);
    let next = at + wait;
    assert_eq!(dials(&tick(core, next)), 1);
    (wait, next + 60)
}

#[test]
fn failures_to_one_peer_back_off_past_the_quick_redial_cap() {
    let mut core = wanted_and_dialling();
    let (mut waits, mut at) = (Vec::new(), 60);
    for _ in 0..8 {
        let (wait, next) = fail_and_redial(&mut core, at);
        waits.push(wait);
        at = next;
    }
    // Two quick redials (1 s, 2 s), then the unreachable schedule, doubling to its cap.
    assert_eq!(waits, [1, 2, 60, 120, 240, 480, 900, 900]);
}

#[test]
fn word_that_the_peer_is_reachable_dials_it_at_once() {
    let mut core = wanted_and_dialling();
    let mut at = 60;
    for _ in 0..5 {
        at = fail_and_redial(&mut core, at).1;
    }
    failed(&mut core, at);
    assert_eq!(dials(&tick(&mut core, at + 1)), 0, "still backing off");
    let told = core.step(Input::PeerReachable {
        now: secs(at + 2),
        peer: PEER,
    });
    assert_eq!(dials(&told), 1, "{told:?}");
    // And the schedule starts over: the next failure waits the quick 1 s.
    assert_eq!(wait_after_failure(&mut core, at + 62), 1);
}

#[test]
fn word_about_a_peer_nobody_wants_dials_nothing() {
    let mut core = core(QUIET);
    let told = core.step(Input::PeerReachable {
        now: secs(1),
        peer: PEER,
    });
    assert_eq!(dials(&told), 0, "{told:?}");
    assert_eq!(dials(&tick(&mut core, 3_600)), 0);
}

#[test]
fn a_window_repeating_its_interest_does_not_shorten_the_wait() {
    let mut core = wanted_and_dialling();
    let mut at = 60;
    for _ in 0..4 {
        at = fail_and_redial(&mut core, at).1;
    }
    failed(&mut core, at);
    let due = core.next_wake();
    let told = core.step(Input::Interest {
        peer: PEER,
        until: secs(at + 100),
    });
    assert_eq!(dials(&told), 0, "{told:?}");
    assert_eq!(core.next_wake(), due);
}

#[test]
fn a_connection_forgives_the_failures_before_it() {
    let mut core = wanted_and_dialling();
    let mut at = 60;
    for _ in 0..4 {
        at = fail_and_redial(&mut core, at).1;
    }
    core.step(Input::DialResult {
        now: secs(at),
        peer: PEER,
        outcome: DialOutcome::Connected,
    });
    up(&mut core, at, P2pPathReport::ServerRelay);
    // Held past `stable_after`, then lost: redialled at once, and a failure waits 1 s again.
    assert_eq!(dials(&lost(&mut core, at + 61)), 1);
    assert_eq!(wait_after_failure(&mut core, at + 121), 1);
}

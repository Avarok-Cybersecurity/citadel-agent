//! Dialling: who is wanted, when, and the backoff between attempts.

use super::super::policy::SupervisorPolicy;
use super::super::types::{Command, DialOutcome, Input, LinkStatus, SupervisorEvent};
use super::tests::{core, secs, tick, PEER, TEST_POLICY};
use citadel_internal_service_types::P2pPathReport;
use std::time::Duration;

/// Probing is not what these tests are about: pushed a day out, it never interleaves.
pub(super) const QUIET: SupervisorPolicy = SupervisorPolicy {
    probe_interval: Duration::from_secs(86_400),
    ..TEST_POLICY
};

pub(super) fn up(core: &mut super::Core, at: u64, path: P2pPathReport) {
    core.step(Input::PeerUp {
        now: secs(at),
        peer: PEER,
        path,
    });
}

pub(super) fn lost(core: &mut super::Core, at: u64) -> Vec<Command> {
    core.step(Input::PeerLost {
        now: secs(at),
        peer: PEER,
    })
}

pub(super) fn failed(core: &mut super::Core, at: u64) -> Vec<Command> {
    core.step(Input::DialResult {
        now: secs(at),
        peer: PEER,
        outcome: DialOutcome::Failed,
    })
}

pub(super) fn dials(commands: &[Command]) -> usize {
    commands
        .iter()
        .filter(|c| matches!(c, Command::DialPeer { .. }))
        .count()
}

pub(super) fn upgrades(commands: &[Command]) -> usize {
    commands
        .iter()
        .filter(|c| matches!(c, Command::UpgradePath { .. }))
        .count()
}

#[test]
fn a_peer_nobody_wants_is_not_dialled() {
    let mut core = core(TEST_POLICY);
    let known = core.step(Input::Backlog {
        peer: PEER,
        pending: 0,
    });
    assert_eq!(dials(&known), 0);
    assert_eq!(
        dials(&tick(&mut core, 5)),
        0,
        "known, but nothing queued and nobody asking"
    );
}

#[test]
fn a_queued_message_makes_its_peer_wanted() {
    let mut core = core(TEST_POLICY);
    let told = core.step(Input::Backlog {
        peer: PEER,
        pending: 3,
    });
    assert_eq!(dials(&told), 1, "{told:?}");
    core.step(Input::Backlog {
        peer: PEER,
        pending: 0,
    });
    failed(&mut core, 6);
    assert_eq!(
        dials(&tick(&mut core, 600)),
        0,
        "nothing queued, nobody asking, never lost"
    );
}

#[test]
fn an_open_window_makes_its_peer_wanted_until_it_expires() {
    let mut core = core(TEST_POLICY);
    let told = core.step(Input::Interest {
        peer: PEER,
        until: secs(30),
    });
    assert_eq!(dials(&told), 1, "dialled as soon as it is wanted: {told:?}");
    failed(&mut core, 2);
    assert_eq!(dials(&tick(&mut core, 40)), 0, "the window closed");
}

#[test]
fn a_lost_peer_is_redialled_with_a_doubling_backoff() {
    let mut core = core(QUIET);
    up(&mut core, 1, P2pPathReport::Direct);
    assert_eq!(dials(&lost(&mut core, 10)), 0, "up for 9 s: it waits 1 s");
    assert_eq!(dials(&tick(&mut core, 11)), 1);
    failed(&mut core, 12);
    assert_eq!(dials(&tick(&mut core, 13)), 0);
    assert_eq!(dials(&tick(&mut core, 14)), 1, "2 s after the second");
    failed(&mut core, 15);
    assert_eq!(dials(&tick(&mut core, 18)), 0);
    assert_eq!(dials(&tick(&mut core, 19)), 1, "4 s after the third");
}

#[test]
fn the_backoff_is_capped() {
    let mut core = core(QUIET);
    up(&mut core, 1, P2pPathReport::Direct);
    lost(&mut core, 10);
    let mut gaps = Vec::new();
    let mut at = 11;
    for _ in 0..8 {
        assert_eq!(dials(&tick(&mut core, at)), 1, "due at {at}");
        failed(&mut core, at);
        let next = core.next_wake().expect("a redial is scheduled").0 / 1000;
        gaps.push(next - at);
        at = next;
    }
    assert_eq!(gaps, vec![2, 4, 8, 16, 30, 30, 30, 30]);
}

#[test]
fn a_peer_seen_up_is_redialled_only_for_the_redial_window() {
    let mut core = core(QUIET);
    up(&mut core, 1, P2pPathReport::Direct);
    lost(&mut core, 10);
    assert_eq!(dials(&tick(&mut core, 11)), 1);
    failed(&mut core, 12);
    assert_eq!(
        dials(&tick(&mut core, 700)),
        0,
        "600 s after the loss it is let go"
    );
}

#[test]
fn nothing_is_dialled_while_the_server_link_is_down() {
    let mut core = core(TEST_POLICY);
    up(&mut core, 1, P2pPathReport::Direct);
    core.step(Input::ServerLink {
        now: secs(50),
        state: LinkStatus::Reconnecting,
    });
    lost(&mut core, 50);
    assert_eq!(dials(&tick(&mut core, 80)), 0);
    assert_eq!(core.next_wake(), None);
    let back = core.step(Input::ServerLink {
        now: secs(90),
        state: LinkStatus::Up,
    });
    assert_eq!(
        dials(&back),
        1,
        "dialled the moment the link is up: {back:?}"
    );
}

#[test]
fn a_supervisor_that_does_not_dial_never_does() {
    let mut policy = TEST_POLICY;
    policy.dial_peers = false;
    let mut core = core(policy);
    core.step(Input::Backlog {
        peer: PEER,
        pending: 9,
    });
    core.step(Input::Interest {
        peer: PEER,
        until: secs(500),
    });
    up(&mut core, 1, P2pPathReport::Direct);
    lost(&mut core, 10);
    for at in [11, 40, 100, 300] {
        assert_eq!(dials(&tick(&mut core, at)), 0, "at {at}");
    }
    assert!(
        core.next_wake().is_none_or(|due| due >= secs(15)),
        "only the probe wakes it"
    );
}

#[test]
fn a_failed_dial_is_reported_once_until_the_peer_is_back() {
    let mut core = core(TEST_POLICY);
    core.step(Input::Backlog {
        peer: PEER,
        pending: 1,
    });
    tick(&mut core, 1);
    let first = failed(&mut core, 2);
    assert!(first.contains(&Command::Report(SupervisorEvent::Degraded {
        peer_cid: PEER
    })));
    assert!(!failed(&mut core, 5)
        .iter()
        .any(|c| matches!(c, Command::Report(_))));
}

#[test]
fn a_restored_peer_is_reported_with_its_path() {
    let mut core = core(TEST_POLICY);
    up(&mut core, 1, P2pPathReport::Direct);
    lost(&mut core, 10);
    let told = core.step(Input::PeerUp {
        now: secs(12),
        peer: PEER,
        path: P2pPathReport::ServerRelay,
    });
    assert!(
        told.contains(&Command::Report(SupervisorEvent::PeerRestored {
            peer_cid: PEER,
            path: P2pPathReport::ServerRelay
        }))
    );
}

#[test]
fn a_dial_that_connected_is_not_dialled_again_before_its_path_is_reported() {
    let mut core = core(QUIET);
    let told = core.step(Input::Interest {
        peer: PEER,
        until: secs(100_000),
    });
    assert_eq!(dials(&told), 1);
    let answered = core.step(Input::DialResult {
        now: secs(1),
        peer: PEER,
        outcome: DialOutcome::Connected,
    });
    assert_eq!(dials(&answered), 0);
    assert_eq!(
        dials(&tick(&mut core, 2)),
        0,
        "it is connected, over the relay"
    );
    assert_eq!(dials(&tick(&mut core, 1000)), 0);
}

//! Path healing, flapping, the backoff's reset and a peer the user let go.

use super::super::types::{Command, Input, Millis, ProbeOutcome};
use super::tests::{core, secs, tick, PEER, TEST_POLICY};
use super::tests_peers::{dials, failed, lost, up, upgrades, QUIET};
use citadel_internal_service_types::P2pPathReport;

#[test]
fn a_relayed_peer_in_use_is_upgraded_again_and_again_with_growing_backoff() {
    let mut core = core(QUIET);
    core.step(Input::Interest {
        peer: PEER,
        until: secs(100_000),
    });
    up(&mut core, 1, P2pPathReport::ServerRelay);
    assert_eq!(
        upgrades(&tick(&mut core, 5)),
        0,
        "the SDK's own campaign goes first"
    );
    let mut due = 6;
    let mut gaps = Vec::new();
    for _ in 0..6 {
        let told = tick(&mut core, due);
        assert_eq!(upgrades(&told), 1, "due at {due}");
        assert!(told.contains(&Command::UpgradePath {
            peer: PEER,
            restore_udp: true
        }));
        let next = core.next_wake().expect("another upgrade is scheduled");
        gaps.push(next.0 / 1000 - due);
        due = next.0 / 1000;
    }
    assert_eq!(
        gaps,
        vec![10, 20, 40, 80, 160, 300],
        "doubling, then capped at 5 minutes"
    );
}

#[test]
fn a_relayed_peer_nobody_uses_is_left_to_the_sdk() {
    let mut core = core(TEST_POLICY);
    up(&mut core, 1, P2pPathReport::ServerRelay);
    assert_eq!(upgrades(&tick(&mut core, 1000)), 0);
}

#[test]
fn a_direct_peer_is_not_upgraded() {
    let mut core = core(TEST_POLICY);
    core.step(Input::Interest {
        peer: PEER,
        until: secs(100_000),
    });
    up(&mut core, 1, P2pPathReport::Direct);
    assert_eq!(upgrades(&tick(&mut core, 1000)), 0);
}

#[test]
fn a_network_change_rearms_every_relayed_peer_once_the_server_answers() {
    let mut core = core(TEST_POLICY);
    up(&mut core, 1, P2pPathReport::ServerRelay);
    core.step(Input::NetworkChanged { now: secs(30) });
    let told = core.step(Input::ServerProbe {
        now: secs(31),
        outcome: ProbeOutcome::Ok {
            rtt: std::time::Duration::from_millis(30),
        },
    });
    assert_eq!(
        told,
        vec![Command::UpgradePath {
            peer: PEER,
            restore_udp: true
        }]
    );
}

#[test]
fn a_stable_route_forgets_its_backoff() {
    let mut core = core(TEST_POLICY);
    core.step(Input::Interest {
        peer: PEER,
        until: secs(100_000),
    });
    up(&mut core, 1, P2pPathReport::Direct);
    lost(&mut core, 5);
    tick(&mut core, 6);
    failed(&mut core, 6);
    failed(&mut core, 8);
    up(&mut core, 20, P2pPathReport::Direct);
    tick(&mut core, 81);
    assert_eq!(
        dials(&lost(&mut core, 82)),
        1,
        "held for a minute: redialled at once"
    );
}

#[test]
fn a_route_that_flaps_keeps_its_backoff() {
    let mut core = core(TEST_POLICY);
    core.step(Input::Interest {
        peer: PEER,
        until: secs(100_000),
    });
    up(&mut core, 1, P2pPathReport::Direct);
    lost(&mut core, 5);
    up(&mut core, 7, P2pPathReport::Direct);
    assert_eq!(
        dials(&lost(&mut core, 9)),
        0,
        "lost again within a minute: it waits"
    );
    assert_eq!(dials(&tick(&mut core, 11)), 1);
}

#[test]
fn jitter_is_bounded_and_only_shortens() {
    let mut policy = TEST_POLICY;
    policy.jitter_permille = 250;
    let mut core = core(policy);
    up(&mut core, 1, P2pPathReport::Direct);
    lost(&mut core, 5);
    let due = core.next_wake().expect("a redial is scheduled");
    assert!(due >= Millis(5750) && due <= secs(6), "{due:?}");
}

#[test]
fn a_peer_the_user_released_is_not_redialled() {
    let mut core = core(TEST_POLICY);
    core.step(Input::Interest {
        peer: PEER,
        until: secs(100_000),
    });
    up(&mut core, 1, P2pPathReport::Direct);
    core.step(Input::PeerReleased {
        now: secs(50),
        peer: PEER,
    });
    assert_eq!(dials(&tick(&mut core, 60)), 0);
    assert_eq!(dials(&tick(&mut core, 600)), 0);
}

#[test]
fn a_released_peer_with_mail_queued_is_still_reached() {
    let mut core = core(TEST_POLICY);
    up(&mut core, 1, P2pPathReport::Direct);
    core.step(Input::PeerReleased {
        now: secs(50),
        peer: PEER,
    });
    core.step(Input::Backlog {
        peer: PEER,
        pending: 2,
    });
    assert_eq!(dials(&tick(&mut core, 51)), 1);
}

#[test]
fn a_path_that_held_makes_the_next_fall_back_start_the_upgrade_backoff_over() {
    let mut core = core(QUIET);
    core.step(Input::Interest {
        peer: PEER,
        until: secs(100_000),
    });
    up(&mut core, 1, P2pPathReport::ServerRelay);
    for at in [6, 16, 36] {
        assert_eq!(upgrades(&tick(&mut core, at)), 1, "due at {at}");
    }
    core.step(Input::PeerPath {
        now: secs(40),
        peer: PEER,
        path: P2pPathReport::Direct,
    });
    tick(&mut core, 101);
    core.step(Input::PeerPath {
        now: secs(102),
        peer: PEER,
        path: P2pPathReport::ServerRelay,
    });
    assert_eq!(
        core.next_wake(),
        Some(secs(107)),
        "the first backoff again, not the fourth"
    );
}

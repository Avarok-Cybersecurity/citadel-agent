//! The core's rules, driven by a manual clock: time is whatever the test stamps on an input.

use super::super::policy::{Backoff, SupervisorPolicy};
use super::super::types::{
    Command, HealCause, Input, LinkStatus, Millis, ProbeOutcome, SupervisorEvent,
};
use super::Core;
use std::time::Duration;

/// Fixed numbers, not the agent's: the rules are what is under test. Jitter is off so a
/// backoff is exact; `jitter_is_bounded_and_only_shortens` turns it on.
pub(super) const TEST_POLICY: SupervisorPolicy = SupervisorPolicy {
    probe_interval: Duration::from_secs(15),
    probe_timeout: Duration::from_secs(5),
    missed_probes: 2,
    dial_backoff: Backoff {
        initial: Duration::from_secs(1),
        max: Duration::from_secs(30),
    },
    upgrade_backoff: Backoff {
        initial: Duration::from_secs(5),
        max: Duration::from_secs(300),
    },
    stable_after: Duration::from_secs(60),
    redial_window: Duration::from_secs(600),
    interest_ceiling: Duration::from_secs(120),
    backlog_poll: Duration::from_secs(2),
    jitter_seed: 7,
    jitter_permille: 0,
    dial_peers: true,
};

pub(super) const PEER: u64 = 11;

pub(super) fn secs(s: u64) -> Millis {
    Millis(s * 1000)
}

pub(super) fn core(policy: SupervisorPolicy) -> Core {
    let mut core = Core::new(policy, 1);
    // The first input says what time it is; the probe is armed from it.
    assert!(core.step(Input::Tick { now: secs(0) }).is_empty());
    core
}

pub(super) fn timeout(core: &mut Core, at: u64) -> Vec<Command> {
    core.step(Input::ServerProbe {
        now: secs(at),
        outcome: ProbeOutcome::Timeout,
    })
}

pub(super) fn tick(core: &mut Core, at: u64) -> Vec<Command> {
    core.step(Input::Tick { now: secs(at) })
}

#[test]
fn a_quiet_link_is_probed_each_interval() {
    let mut core = core(TEST_POLICY);
    assert!(tick(&mut core, 14).is_empty());
    assert_eq!(tick(&mut core, 15), vec![Command::ProbeServer]);
    assert!(tick(&mut core, 16).is_empty(), "one probe at a time");
    assert_eq!(core.next_wake(), None, "waiting on the probe's answer");
    core.step(Input::ServerProbe {
        now: secs(17),
        outcome: ProbeOutcome::Ok {
            rtt: Duration::from_millis(40),
        },
    });
    assert_eq!(core.next_wake(), Some(secs(32)));
    assert_eq!(tick(&mut core, 32), vec![Command::ProbeServer]);
}

#[test]
fn two_missed_probes_end_the_link() {
    let mut core = core(TEST_POLICY);
    assert_eq!(tick(&mut core, 15), vec![Command::ProbeServer]);
    assert_eq!(timeout(&mut core, 20), vec![Command::ProbeServer]);
    assert_eq!(
        timeout(&mut core, 25),
        vec![
            Command::ForceReconnect,
            Command::Report(SupervisorEvent::Healing {
                cause: HealCause::ProbesMissed
            })
        ]
    );
    assert!(
        tick(&mut core, 200).is_empty(),
        "no probing while reconnecting"
    );
}

#[test]
fn an_answered_probe_forgives_a_miss() {
    let mut core = core(TEST_POLICY);
    tick(&mut core, 15);
    assert_eq!(timeout(&mut core, 20), vec![Command::ProbeServer]);
    core.step(Input::ServerProbe {
        now: secs(21),
        outcome: ProbeOutcome::Ok {
            rtt: Duration::from_millis(900),
        },
    });
    tick(&mut core, 36);
    assert_eq!(
        timeout(&mut core, 41),
        vec![Command::ProbeServer],
        "the count started over"
    );
}

#[test]
fn a_probe_that_could_not_be_made_is_not_a_miss() {
    let mut core = core(TEST_POLICY);
    for round in 0..4u64 {
        let at = core.next_wake().expect("the next probe is scheduled").0 / 1000;
        assert_eq!(tick(&mut core, at), vec![Command::ProbeServer]);
        let answer = core.step(Input::ServerProbe {
            now: secs(at + 1),
            outcome: ProbeOutcome::Error,
        });
        assert!(answer.is_empty(), "round {round}: {answer:?}");
    }
    let at = core.next_wake().expect("the next probe is scheduled").0 / 1000;
    tick(&mut core, at);
    assert_eq!(
        timeout(&mut core, at + 1),
        vec![Command::ProbeServer],
        "the errors counted for nothing: this is the first miss"
    );
}

#[test]
fn a_network_change_rebinds_then_probes_at_once() {
    let mut core = core(TEST_POLICY);
    assert_eq!(
        core.step(Input::NetworkChanged { now: secs(3) }),
        vec![Command::RebindTransports, Command::ProbeServer]
    );
}

#[test]
fn a_dead_path_after_a_network_change_ends_the_link_within_two_timeouts() {
    let mut core = core(TEST_POLICY);
    core.step(Input::NetworkChanged { now: secs(3) });
    assert_eq!(timeout(&mut core, 8), vec![Command::ProbeServer]);
    assert!(timeout(&mut core, 13).contains(&Command::ForceReconnect));
}

#[test]
fn a_network_change_during_a_probe_probes_again_whatever_the_old_one_says() {
    let mut core = core(TEST_POLICY);
    tick(&mut core, 15);
    assert_eq!(
        core.step(Input::NetworkChanged { now: secs(16) }),
        vec![Command::RebindTransports]
    );
    let again = timeout(&mut core, 20);
    assert_eq!(again, vec![Command::ProbeServer], "not counted as a miss");
    assert_eq!(timeout(&mut core, 25), vec![Command::ProbeServer]);
}

#[test]
fn a_network_change_does_not_alarm_the_windows_by_itself() {
    let mut core = core(TEST_POLICY);
    let told = core.step(Input::NetworkChanged { now: secs(3) });
    assert!(
        !told.iter().any(|c| matches!(c, Command::Report(_))),
        "{told:?}"
    );
}

#[test]
fn the_server_link_coming_back_is_reported_healed() {
    let mut core = core(TEST_POLICY);
    let lost = core.step(Input::ServerLink {
        now: secs(1),
        state: LinkStatus::Reconnecting,
    });
    assert_eq!(
        lost,
        vec![Command::Report(SupervisorEvent::Healing {
            cause: HealCause::LinkLost
        })]
    );
    assert_eq!(
        core.step(Input::ServerLink {
            now: secs(5),
            state: LinkStatus::Up
        }),
        vec![Command::Report(SupervisorEvent::Healed)]
    );
    assert_eq!(
        core.next_wake(),
        Some(secs(20)),
        "probing resumes from the new link"
    );
}

#[test]
fn an_ended_session_is_inert_for_good() {
    let mut core = core(TEST_POLICY);
    core.step(Input::ServerLink {
        now: secs(1),
        state: LinkStatus::Ended,
    });
    assert!(core.step(Input::NetworkChanged { now: secs(2) }).is_empty());
    assert!(tick(&mut core, 500).is_empty());
    assert_eq!(core.next_wake(), None);
}

//! A forced link that could not be ended, and one that then dropped.

use super::super::types::{Command, Input, LinkStatus, ProbeOutcome, SupervisorEvent};
use super::tests::{core, secs, tick, timeout, TEST_POLICY};
use std::time::Duration;

#[test]
fn a_link_that_could_not_be_ended_is_probed_again_and_healed_by_an_answer() {
    let mut core = core(TEST_POLICY);
    tick(&mut core, 15);
    timeout(&mut core, 20);
    assert!(timeout(&mut core, 25).contains(&Command::ForceReconnect));
    assert_eq!(core.next_wake(), None, "waiting on the force");
    assert_eq!(
        core.step(Input::NetworkChanged { now: secs(26) }),
        vec![Command::RebindTransports]
    );
    assert!(core.step(Input::ForceFailed { now: secs(30) }).is_empty());
    assert_eq!(core.next_wake(), Some(secs(45)));
    assert_eq!(tick(&mut core, 45), vec![Command::ProbeServer]);
    let healed = core.step(Input::ServerProbe {
        now: secs(46),
        outcome: ProbeOutcome::Ok {
            rtt: Duration::from_millis(30),
        },
    });
    assert_eq!(healed, vec![Command::Report(SupervisorEvent::Healed)]);
}

#[test]
fn a_forced_link_that_then_drops_is_not_announced_twice() {
    let mut core = core(TEST_POLICY);
    tick(&mut core, 15);
    timeout(&mut core, 20);
    timeout(&mut core, 25);
    let dropped = core.step(Input::ServerLink {
        now: secs(26),
        state: LinkStatus::Reconnecting,
    });
    assert!(dropped.is_empty(), "Healing was already said: {dropped:?}");
    assert_eq!(
        core.step(Input::ServerLink {
            now: secs(30),
            state: LinkStatus::Up
        }),
        vec![Command::Report(SupervisorEvent::Healed)]
    );
}

//! A force is about the link its probes went unanswered on. The SDK abandons by CID, so a
//! link a reconnect installed after those probes would be ended in its place. The decision is
//! the one `adapters/server_link.rs` makes before abandoning (`probed_link_is_current`), and
//! its refusal reaches the core as `ForceFailed`, as the shell sends it.

use super::super::types::{Command, HealCause, Input, LinkStatus, ProbeOutcome, SupervisorEvent};
use super::tests::{core, secs, tick, timeout, TEST_POLICY};
use crate::kernel::reconnect::instance::{probed_link_is_current, Instance};
use crate::kernel::reconnect::LinkState;
use citadel_sdk::prelude::Ticket;
use std::time::Duration;

const PROBED: Instance = Ticket(337_726_653);
const REPLACEMENT: Instance = Ticket(218_601_668);

#[test]
fn a_force_for_a_replaced_link_leaves_the_replacement_alone() {
    let mut core = core(TEST_POLICY);
    tick(&mut core, 15);
    timeout(&mut core, 20);
    let forced = timeout(&mut core, 25);
    assert!(forced.contains(&Command::ForceReconnect), "{forced:?}");
    assert!(forced.contains(&Command::Report(SupervisorEvent::Healing {
        cause: HealCause::ProbesMissed
    })));

    // Before the force runs, the probed link's own drop is reported and a reconnect
    // installs another.
    core.step(Input::ServerLink {
        now: secs(26),
        state: LinkStatus::Reconnecting,
    });
    let back = core.step(Input::ServerLink {
        now: secs(27),
        state: LinkStatus::Up,
    });
    assert_eq!(back, vec![Command::Report(SupervisorEvent::Healed)]);

    // The force now finds a link it did not probe.
    let ended = probed_link_is_current(Some((LinkState::Up, REPLACEMENT)), PROBED);
    assert!(!ended, "the replacement would have been abandoned");
    assert!(core.step(Input::ForceFailed { now: secs(28) }).is_empty());

    // The replacement is probed like any link, and answers: nothing more is said.
    assert_eq!(tick(&mut core, 43), vec![Command::ProbeServer]);
    let answered = core.step(Input::ServerProbe {
        now: secs(44),
        outcome: ProbeOutcome::Ok {
            rtt: Duration::from_millis(20),
        },
    });
    assert!(answered.is_empty(), "{answered:?}");
}

#[test]
fn only_the_probed_link_while_it_is_up_is_abandoned() {
    assert!(probed_link_is_current(
        Some((LinkState::Up, PROBED)),
        PROBED
    ));
    assert!(
        !probed_link_is_current(Some((LinkState::Up, REPLACEMENT)), PROBED),
        "replaced since it was probed"
    );
    for link in [
        LinkState::Reconnecting,
        LinkState::SigningIn,
        LinkState::Ending,
    ] {
        assert!(
            !probed_link_is_current(Some((link, PROBED)), PROBED),
            "{link:?}: already being replaced or ended"
        );
    }
    assert!(!probed_link_is_current(None, PROBED), "the session is gone");
}

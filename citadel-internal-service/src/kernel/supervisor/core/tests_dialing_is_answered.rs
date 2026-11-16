//! Every "dialling" a window is told is answered when the peer comes up.
//!
//! A window reads `PeerDialing` as "Reconnecting…" and clears it only on the peer's healed
//! report (`PeerRestored`). That report was sent only for a peer lost while connected; a
//! peer dialled for any other reason -- released by the user and then wanted again (a
//! chat's encryption level changing drops the link and asks for it back), or never
//! connected yet -- came up with no report, and the window said "Reconnecting…" for good.

use super::super::types::{Command, DialOutcome, Input, SupervisorEvent};
use super::tests::{core, secs, tick, PEER};
use super::tests_peers::{up, QUIET};
use citadel_internal_service_types::P2pPathReport;

fn dialing(commands: &[Command]) -> bool {
    commands.contains(&Command::Report(SupervisorEvent::PeerDialing {
        peer_cid: PEER,
    }))
}

fn restored(commands: &[Command]) -> bool {
    commands
        .iter()
        .any(|c| matches!(c, Command::Report(SupervisorEvent::PeerRestored { peer_cid, .. }) if *peer_cid == PEER))
}

fn wanted(core: &mut super::Core) -> Vec<Command> {
    core.step(Input::Interest {
        peer: PEER,
        until: secs(100_000),
    })
}

#[test]
fn a_released_peer_dialled_again_is_reported_restored_when_it_connects() {
    let mut core = core(QUIET);
    wanted(&mut core);
    up(&mut core, 1, P2pPathReport::Direct);
    core.step(Input::PeerReleased {
        now: secs(100),
        peer: PEER,
    });
    let asked = wanted(&mut core);
    assert!(
        dialing(&asked),
        "the window is told the peer is being dialled"
    );
    let answered = core.step(Input::DialResult {
        now: secs(101),
        peer: PEER,
        outcome: DialOutcome::Connected,
    });
    assert!(
        restored(&answered),
        "a dial the window was told of is answered: {answered:?}"
    );
}

#[test]
fn a_released_peer_that_dials_us_first_is_reported_restored() {
    let mut core = core(QUIET);
    wanted(&mut core);
    up(&mut core, 1, P2pPathReport::Direct);
    core.step(Input::PeerReleased {
        now: secs(100),
        peer: PEER,
    });
    assert!(dialing(&wanted(&mut core)));
    let came_up = core.step(Input::PeerUp {
        now: secs(102),
        peer: PEER,
        path: P2pPathReport::ServerRelay,
    });
    assert!(restored(&came_up), "{came_up:?}");
}

#[test]
fn a_first_dial_is_answered_too() {
    let mut core = core(QUIET);
    assert!(dialing(&wanted(&mut core)));
    let answered = core.step(Input::DialResult {
        now: secs(1),
        peer: PEER,
        outcome: DialOutcome::Connected,
    });
    assert!(restored(&answered), "{answered:?}");
}

#[test]
fn a_degraded_report_is_answered_when_the_peer_connects() {
    let mut core = core(QUIET);
    wanted(&mut core);
    core.step(Input::DialResult {
        now: secs(1),
        peer: PEER,
        outcome: DialOutcome::Failed,
    });
    tick(&mut core, 30);
    let came_up = core.step(Input::PeerUp {
        now: secs(31),
        peer: PEER,
        path: P2pPathReport::Direct,
    });
    assert!(restored(&came_up), "{came_up:?}");
}

#[test]
fn a_connection_nobody_was_told_about_reports_nothing() {
    let mut core = core(QUIET);
    let came_up = core.step(Input::PeerUp {
        now: secs(1),
        peer: PEER,
        path: P2pPathReport::Direct,
    });
    assert!(
        !came_up.iter().any(|c| matches!(c, Command::Report(_))),
        "{came_up:?}"
    );
}

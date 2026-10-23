//! A dial refused because an exchange with the peer is already running (the peer's own dial
//! crossed ours). The running exchange connects; the refusal is neither a failure nor a
//! connection.

use super::super::types::{Command, DialOutcome, Input, SupervisorEvent};
use super::tests::{core, secs, tick, PEER};
use super::tests_peers::{dials, up, QUIET};
use super::Core;
use citadel_internal_service_types::P2pPathReport;

fn in_progress(core: &mut Core, at: u64) -> Vec<Command> {
    core.step(Input::DialResult {
        now: secs(at),
        peer: PEER,
        outcome: DialOutcome::InProgress,
    })
}

fn degraded(commands: &[Command]) -> bool {
    commands
        .iter()
        .any(|c| matches!(c, Command::Report(SupervisorEvent::Degraded { .. })))
}

fn wanted(core: &mut Core) {
    let told = core.step(Input::Interest {
        peer: PEER,
        until: secs(3_600),
    });
    assert_eq!(dials(&told), 1, "{told:?}");
}

#[test]
fn a_dial_refused_for_a_running_exchange_does_not_degrade_the_peer() {
    let mut core = core(QUIET);
    wanted(&mut core);
    let answered = in_progress(&mut core, 1);
    assert!(!degraded(&answered), "{answered:?}");
    up(&mut core, 2, P2pPathReport::Direct);
    assert_eq!(
        dials(&tick(&mut core, 30)),
        0,
        "the running exchange connected it"
    );
}

#[test]
fn refusals_while_an_exchange_runs_never_count_toward_unreachable() {
    let mut core = core(QUIET);
    wanted(&mut core);
    let mut at = 1;
    // Three failures would put the peer on the unreachable schedule (60 s); refusals do not.
    for _ in 0..5 {
        assert!(!degraded(&in_progress(&mut core, at)));
        let next = core.next_wake().expect("looked at again").0 / 1000;
        assert_eq!(next - at, 1, "the first backoff, unescalated");
        at = next;
        assert_eq!(dials(&tick(&mut core, at)), 1, "due at {at}");
    }
}

#[test]
fn a_refusal_is_not_a_connection() {
    let mut core = core(QUIET);
    wanted(&mut core);
    in_progress(&mut core, 1);
    assert_eq!(
        dials(&tick(&mut core, 2)),
        1,
        "no PeerUp came, so it is still wanted and dialled again"
    );
}

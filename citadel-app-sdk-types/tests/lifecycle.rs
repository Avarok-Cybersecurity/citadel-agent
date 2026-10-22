//! The lifecycle state machine, held to the design's literal table: every legal step, and every
//! other (state, step) pair refused. The host and the guest SDK both run this one table.

use citadel_app_sdk_types::abi::{CloseResult, REFUSED_CLOSED, REFUSED_CLOSING};
use citadel_app_sdk_types::lifecycle::{IllegalTransition, CLOSING_GROUPS};
use citadel_app_sdk_types::{CloseReason, Group, LifecycleState as S, LifecycleStep as E};

const STEPS: [E; 7] = [
    E::Initialised,
    E::Start,
    E::Suspend,
    E::Resume,
    E::Close,
    E::Settled,
    E::Abort,
];

/// The design's table, written out: (from, step, to). Nothing else is legal.
const LEGAL: [(S, E, S); 13] = [
    (S::Created, E::Initialised, S::Initialised),
    (S::Created, E::Abort, S::Closed),
    (S::Initialised, E::Start, S::Running),
    (S::Initialised, E::Close, S::Closing),
    (S::Initialised, E::Abort, S::Closed),
    (S::Running, E::Suspend, S::Suspended),
    (S::Running, E::Close, S::Closing),
    (S::Running, E::Abort, S::Closed),
    (S::Suspended, E::Resume, S::Running),
    (S::Suspended, E::Close, S::Closing),
    (S::Suspended, E::Abort, S::Closed),
    (S::Closing, E::Settled, S::Closed),
    (S::Closing, E::Abort, S::Closed),
];

#[test]
fn the_table_is_exactly_the_designed_one() {
    for from in S::ALL {
        for step in STEPS {
            let want = LEGAL
                .iter()
                .find(|(f, e, _)| *f == from && *e == step)
                .map(|(_, _, to)| *to);
            assert_eq!(from.after(step), want, "{from:?} --{step:?}-->");
        }
    }
}

#[test]
fn an_illegal_step_is_refused_and_names_where_it_was_tried() {
    let illegal = [
        (S::Running, E::Resume),
        (S::Suspended, E::Suspend),
        (S::Running, E::Start),
        (S::Closing, E::Close),
        (S::Created, E::Close),
        (S::Created, E::Start),
        (S::Running, E::Settled),
        (S::Closed, E::Abort),
        (S::Closed, E::Close),
    ];
    for (from, step) in illegal {
        assert_eq!(from.step(step), Err(IllegalTransition { from, step }));
    }
    // Control: the same steps from the states that allow them.
    assert_eq!(S::Suspended.step(E::Resume), Ok(S::Running));
    assert_eq!(S::Running.step(E::Close), Ok(S::Closing));
}

#[test]
fn nothing_leaves_closed() {
    for step in STEPS {
        assert_eq!(S::Closed.after(step), None, "{step:?}");
    }
}

#[test]
fn a_closing_app_may_persist_and_log_and_a_closed_one_nothing() {
    assert_eq!(CLOSING_GROUPS, [Group::Storage, Group::Log]);
    for group in Group::ALL {
        let closing = S::Closing.refusal_for(*group);
        let allowed = CLOSING_GROUPS.contains(group);
        assert_eq!(closing, (!allowed).then_some(REFUSED_CLOSING), "{group:?}");
        assert_eq!(S::Closed.refusal_for(*group), Some(REFUSED_CLOSED));
        for live in [S::Created, S::Initialised, S::Running, S::Suspended] {
            assert_eq!(live.refusal_for(*group), None, "{live:?} {group:?}");
        }
    }
}

#[test]
fn close_reasons_are_stable_codes_and_close_results_too() {
    let codes: Vec<i32> = CloseReason::ALL.iter().map(|r| r.code()).collect();
    assert_eq!(codes, [0, 1, 2, 3, 4, 5]);
    for r in CloseReason::ALL {
        assert_eq!(CloseReason::from_code(r.code()), Some(r));
    }
    assert_eq!(CloseReason::from_code(6), None);
    assert_eq!(CloseReason::from_code(-1), None);
    let results: Vec<i32> = CloseResult::ALL.iter().map(|r| r.code()).collect();
    assert_eq!(results, [0, 1, 2]);
}

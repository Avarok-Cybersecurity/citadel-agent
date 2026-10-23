use super::*;
use citadel_app_sdk_types::calls::{ResponseHead, Via};

fn limits(max_concurrency: u32) -> FanoutLimits {
    FanoutLimits {
        max_concurrency,
        per_call_ms: 100,
        budget_ms: 1000,
        max_total_bytes: 10,
    }
}

fn ok() -> Outcome {
    let head = ResponseHead {
        status: 200,
        headers: vec![],
        body_len: Some(0),
    };
    Outcome::Ok(Response {
        head,
        via: Via::P2p,
        body: vec![],
    })
}

#[test]
fn at_most_the_cap_runs_and_a_settled_target_makes_room_for_the_next() {
    let mut plan = Plan::new(5, limits(2));
    assert_eq!(plan.admit(), [0, 1]);
    assert!(plan.admit().is_empty(), "the cap is full");
    assert!(plan.settle(1, ok()));
    assert_eq!(plan.admit(), [2]);
    assert!(!plan.settle(1, Outcome::Skipped), "settled once");
    assert!(!plan.is_done());
}

#[test]
fn a_call_past_its_own_time_fails_with_the_per_call_limit_and_others_run_on() {
    let mut plan = Plan::new(3, limits(2));
    plan.admit();
    assert!(plan.elapse(60).is_empty());
    plan.settle(0, ok());
    assert_eq!(plan.admit(), [2]);
    let late = plan.elapse(50);
    let refused = Outcome::Failed(Refused::Limit(LIMIT_PER_CALL_MS.into()));
    assert_eq!(late, [(1, refused)], "target 2 has run only 50 ms");
}

#[test]
fn past_the_budget_everything_running_and_waiting_is_skipped_and_the_fanout_ends() {
    let mut plan = Plan::new(4, limits(1));
    plan.admit();
    let stopped = plan.elapse(1000);
    assert_eq!(stopped, [(0, Outcome::Skipped)]);
    for (i, outcome) in stopped {
        plan.settle(i, outcome);
    }
    assert!(plan.is_done());
    assert!(plan.admit().is_empty());
    assert_eq!(plan.outcomes(), vec![Outcome::Skipped; 4]);
}

#[test]
fn the_response_that_takes_the_total_past_the_byte_budget_is_stopped() {
    let mut plan = Plan::new(2, limits(2));
    plan.admit();
    assert_eq!(plan.bytes(0, 6), None);
    assert_eq!(
        plan.bytes(1, 5),
        Some(Refused::Limit(LIMIT_TOTAL_BYTES.into()))
    );
}

#[test]
fn a_zero_limit_admits_nothing_and_settles_every_target_refused() {
    for zero in [
        FanoutLimits {
            max_concurrency: 0,
            ..limits(1)
        },
        FanoutLimits {
            budget_ms: 0,
            ..limits(1)
        },
        FanoutLimits {
            max_total_bytes: 0,
            ..limits(1)
        },
    ] {
        let mut plan = Plan::new(2, zero);
        assert!(plan.admit().is_empty());
        assert!(plan.is_done());
        let refused = Outcome::Failed(Refused::Policy(POLICY_ZERO_LIMIT.into()));
        assert_eq!(plan.outcomes(), vec![refused; 2]);
    }
}

#[test]
fn no_targets_is_done_at_once() {
    assert!(Plan::new(0, limits(1)).is_done());
}

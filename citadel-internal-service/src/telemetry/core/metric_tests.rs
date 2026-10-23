use super::*;
use crate::telemetry::config::RingConfig;
use crate::telemetry::metric_fixture::path_report as report;
use crate::telemetry::redact::{KnownValue, RedactionKind, Salt};

const A: u64 = 1_111;
const B: u64 = 2_222;
const PEER: u64 = 9_999;

fn core(max_entries: usize) -> CollectorCore {
    let mut core = CollectorCore::new(RingConfig::new(max_entries, 1 << 20, 1024).unwrap());
    for (cid, name) in [(A, "alice"), (B, "bob")] {
        let identity = vec![KnownValue {
            kind: RedactionKind::Username,
            value: name.into(),
        }];
        core.register_session(cid, identity);
    }
    core
}

#[test]
fn a_metric_is_kept_only_for_an_opted_in_session() {
    let mut core = core(16);
    assert!(!core.capture_metric(A, 5, PEER, report(1)), "not opted in");
    assert!(
        !core.capture_metric(77, 5, PEER, report(1)),
        "unknown session"
    );
    core.opt_in(A, Salt::from_bytes([1; 32]), true).unwrap();
    assert!(core.capture_metric(A, 5, PEER, report(1)));
    let kept = core.drain(A, Salt::from_bytes([2; 32])).unwrap();
    assert_eq!(kept.len(), 1);
    let entry = &kept[0];
    assert_eq!(entry.level, TelemetryLevel::Metric);
    assert_eq!(entry.target, METRIC_TARGET);
    assert_eq!(entry.peer_cid, Some(PEER), "the peer's CID is kept");
    assert_eq!(entry.metric, Some(report(1)));
    assert!(
        entry.message.contains("DirectThenTurn"),
        "{}",
        entry.message
    );
}

#[test]
fn a_metric_goes_to_its_own_session_only() {
    let mut core = core(16);
    core.opt_in(A, Salt::from_bytes([1; 32]), true).unwrap();
    core.opt_in(B, Salt::from_bytes([1; 32]), true).unwrap();
    assert!(core.capture_metric(A, 5, PEER, report(1)));
    assert!(core.drain(B, Salt::from_bytes([2; 32])).unwrap().is_empty());
}

#[test]
fn metrics_are_bounded_by_the_ring() {
    let mut core = core(2);
    core.opt_in(A, Salt::from_bytes([1; 32]), true).unwrap();
    for attempt in 1..=3 {
        assert!(core.capture_metric(A, 5, PEER, report(attempt)));
    }
    let kept = core.drain(A, Salt::from_bytes([2; 32])).unwrap();
    let attempts: Vec<_> = kept
        .iter()
        .map(|e| match &e.metric {
            Some(TelemetryMetric::PathReport(r)) => r.attempt,
            None => panic!("a metric entry without its metric"),
        })
        .collect();
    assert_eq!(attempts, vec![2, 3], "the oldest is evicted first");
}

#[test]
fn a_server_that_stores_no_metrics_is_sent_none() {
    let mut core = core(16);
    core.opt_in(A, Salt::from_bytes([1; 32]), false).unwrap();
    assert!(!core.capture_metric(A, 5, PEER, report(1)));
    assert!(core.drain(A, Salt::from_bytes([2; 32])).unwrap().is_empty());
    // A later consent answer from a server that does store them turns capture on.
    core.opt_in(A, Salt::from_bytes([1; 32]), true).unwrap();
    assert!(core.capture_metric(A, 5, PEER, report(2)));
}

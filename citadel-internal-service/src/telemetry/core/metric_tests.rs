use super::*;
use crate::telemetry::config::RingConfig;
use crate::telemetry::entry::{PathReport, StageReport};
use crate::telemetry::redact::{KnownValue, RedactionKind, Salt};
use citadel_sdk::prelude::{IpTranslationClass, NatClass, P2pPath, PortTranslationClass};

const A: u64 = 1_111;
const B: u64 = 2_222;
const PEER: u64 = 9_999;

fn nat() -> NatClass {
    NatClass {
        ip_translation: IpTranslationClass::Constant,
        port_translation: PortTranslationClass::Identity,
        ipv6_enabled: false,
    }
}

fn report(attempt: u32) -> TelemetryMetric {
    TelemetryMetric::PathReport(PathReport {
        attempt,
        plan: "DirectThenTurn".into(),
        path: P2pPath::Turn,
        relayed_both: true,
        time_to_connect_ms: Some(900),
        stages: vec![StageReport {
            stage: "Direct".into(),
            failure: Some("PunchFailed".into()),
            elapsed_ms: 400,
        }],
        local_nat: nat(),
        peer_nat: nat(),
    })
}

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
    core.opt_in(A, Salt::from_bytes([1; 32])).unwrap();
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
    core.opt_in(A, Salt::from_bytes([1; 32])).unwrap();
    core.opt_in(B, Salt::from_bytes([1; 32])).unwrap();
    assert!(core.capture_metric(A, 5, PEER, report(1)));
    assert!(core.drain(B, Salt::from_bytes([2; 32])).unwrap().is_empty());
}

#[test]
fn metrics_are_bounded_by_the_ring() {
    let mut core = core(2);
    core.opt_in(A, Salt::from_bytes([1; 32])).unwrap();
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

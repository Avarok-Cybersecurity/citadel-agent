//! The uplink's capability gates: metrics only to a server that stores them, TURN reports only
//! to one that meters them, with the fakes of `tests`.

use super::tests::{sent, uplink, Link, ALICE, BOB};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[tokio::test]
async fn a_metered_session_reports_its_turn_bytes_without_opting_in() {
    let uplink = uplink();
    let bytes = Arc::new(AtomicU64::new(0));
    for cid in [ALICE, BOB] {
        let read = bytes.clone();
        uplink.track_turn(cid, 7, Box::new(move || (read.load(Ordering::SeqCst), 1)));
    }
    assert!(
        uplink.on_server_frame(ALICE, b"metered"),
        "consent is forwarded"
    );
    assert!(
        !uplink.on_server_frame(ALICE, b"recorded"),
        "the answer stops at the agent"
    );
    assert!(
        !uplink.on_server_frame(ALICE, b"over-cap"),
        "so does a refusal"
    );
    bytes.store(500, Ordering::SeqCst);
    let link = Link::default();
    uplink.flush(&link).await;
    // Alice's server meters TURN although she never opted in; Bob's never said it does.
    assert_eq!(sent(&link), vec![(ALICE, "turn 500 1".to_string())]);
    uplink.flush(&link).await;
    assert_eq!(sent(&link).len(), 1, "each byte is reported once");
}

#[test]
fn metrics_are_captured_only_for_a_server_that_stores_them() {
    let uplink = uplink();
    let metric = crate::telemetry::metric_fixture::path_report(1);
    uplink.on_server_frame(ALICE, b"allowed-no-metrics");
    assert!(!uplink
        .collector()
        .capture_metric(ALICE, BOB, metric.clone()));
    uplink.on_server_frame(ALICE, b"allowed");
    assert!(uplink.collector().capture_metric(ALICE, BOB, metric));
}

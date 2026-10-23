use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const A: u64 = 11;
const B: u64 = 22;
const PEER: u64 = 99;

/// A connection's counters the test moves, and a reader over them.
fn counter() -> (Arc<(AtomicU64, AtomicU64)>, TurnReader) {
    let bytes = Arc::new((AtomicU64::new(0), AtomicU64::new(0)));
    let read = bytes.clone();
    let reader: TurnReader =
        Box::new(move || (read.0.load(Ordering::SeqCst), read.1.load(Ordering::SeqCst)));
    (bytes, reader)
}

fn grow(bytes: &(AtomicU64, AtomicU64), tx: u64, rx: u64) {
    bytes.0.fetch_add(tx, Ordering::SeqCst);
    bytes.1.fetch_add(rx, Ordering::SeqCst);
}

#[test]
fn a_metered_session_reports_each_byte_once() {
    let mut ledger = TurnLedger::default();
    ledger.set_metered(A, true);
    let (bytes, reader) = counter();
    ledger.track(A, PEER, reader);
    grow(&bytes, 100, 40);
    assert_eq!(ledger.take(), vec![(A, (100, 40))]);
    assert!(ledger.take().is_empty(), "nothing new");
    grow(&bytes, 5, 0);
    assert_eq!(ledger.take(), vec![(A, (5, 0))]);
}

#[test]
fn a_session_whose_server_meters_nothing_reports_nothing() {
    let mut ledger = TurnLedger::default();
    let (bytes, reader) = counter();
    ledger.track(B, PEER, reader);
    grow(&bytes, 100, 40);
    assert!(ledger.take().is_empty());
    ledger.set_metered(B, true);
    assert_eq!(
        ledger.take(),
        vec![(B, (100, 40))],
        "counted from the first byte"
    );
    ledger.set_metered(B, false);
    grow(&bytes, 1, 1);
    assert!(ledger.take().is_empty());
}

#[test]
fn a_replaced_connection_keeps_its_unreported_bytes() {
    let mut ledger = TurnLedger::default();
    ledger.set_metered(A, true);
    let (old, reader) = counter();
    ledger.track(A, PEER, reader);
    grow(&old, 7, 3);
    let (new, reader) = counter();
    ledger.track(A, PEER, reader);
    grow(&new, 10, 10);
    assert_eq!(ledger.take(), vec![(A, (17, 13))]);
}

#[test]
fn a_forgotten_session_reports_nothing_more() {
    let mut ledger = TurnLedger::default();
    ledger.set_metered(A, true);
    let (bytes, reader) = counter();
    ledger.track(A, PEER, reader);
    grow(&bytes, 9, 9);
    ledger.forget(A);
    assert!(ledger.take().is_empty());
}

#[test]
fn a_large_growth_is_split_under_the_cap() {
    assert_eq!(split_report((25, 4), 10), vec![(10, 4), (10, 0), (5, 0)]);
    assert!(split_report((0, 0), 10).is_empty());
}

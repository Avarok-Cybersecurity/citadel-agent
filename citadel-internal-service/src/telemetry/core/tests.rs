use super::*;
use crate::telemetry::entry::TelemetryLevel;
use crate::telemetry::redact::RedactionKind;

const A: u64 = 1_111;
const B: u64 = 2_222;
const REMOTE: u64 = 9_999;

fn salt(n: u8) -> Salt {
    Salt::from_bytes([n; 32])
}

fn user(name: &str) -> Vec<KnownValue> {
    vec![KnownValue {
        kind: RedactionKind::Username,
        value: name.into(),
    }]
}

fn event(session_cid: Option<u64>, message: &str) -> CapturedEvent<'_> {
    CapturedEvent {
        time_ms: 5,
        level: TelemetryLevel::Warn,
        target: "citadel",
        message,
        session_cid,
        peer_cid: None,
    }
}

fn core(max_entries: usize, max_bytes: usize, max_message: usize) -> CollectorCore {
    let mut core =
        CollectorCore::new(RingConfig::new(max_entries, max_bytes, max_message).unwrap());
    core.register_session(A, user("alice"));
    core.register_session(B, user("bob"));
    core
}

fn messages(core: &mut CollectorCore, cid: u64) -> Vec<String> {
    core.drain(cid, salt(0))
        .unwrap()
        .into_iter()
        .map(|e| e.message)
        .collect()
}

#[test]
fn the_ring_evicts_oldest_at_the_count_bound() {
    let mut core = core(2, 1_000, 100);
    core.opt_in(A, salt(1), true).unwrap();
    for line in ["one", "two", "three"] {
        core.capture(&event(Some(A), line));
    }
    assert_eq!(messages(&mut core, A), ["two", "three"]);
}

#[test]
fn the_ring_evicts_oldest_at_the_byte_bound() {
    // Each entry is 10 bytes of message plus 7 of target: two fit in 40, three do not.
    let mut core = core(100, 40, 20);
    core.opt_in(A, salt(1), true).unwrap();
    for line in ["aaaaaaaaaa", "bbbbbbbbbb", "cccccccccc"] {
        core.capture(&event(Some(A), line));
    }
    assert_eq!(messages(&mut core, A), ["bbbbbbbbbb", "cccccccccc"]);
}

#[test]
fn long_messages_are_cut_on_a_char_boundary() {
    let mut core = core(10, 100, 5);
    core.opt_in(A, salt(1), true).unwrap();
    core.capture(&event(Some(A), "abcdé-more"));
    assert_eq!(messages(&mut core, A), ["abcd"]);
}

#[test]
fn opting_out_discards_the_ring_and_stops_capture() {
    let mut core = core(10, 1_000, 100);
    core.opt_in(A, salt(1), true).unwrap();
    core.capture(&event(Some(A), "kept until opt-out"));
    core.opt_out(A).unwrap();
    assert!(!core.is_capturing());
    core.capture(&event(Some(A), "after"));
    assert_eq!(core.drain(A, salt(0)), Err(TelemetryError::NotOptedIn(A)));
    core.opt_in(A, salt(1), true).unwrap();
    assert!(messages(&mut core, A).is_empty());
}

#[test]
fn unknown_sessions_fail_fast() {
    let mut core = core(10, 1_000, 100);
    assert_eq!(
        core.opt_in(7, salt(1), true),
        Err(TelemetryError::UnknownSession(7))
    );
    assert_eq!(
        core.learn_peer(7, 8, vec![]),
        Err(TelemetryError::UnknownSession(7))
    );
    core.forget_session(A);
    assert_eq!(core.opt_out(A), Err(TelemetryError::UnknownSession(A)));
}

#[test]
fn a_session_line_goes_only_to_that_session() {
    let mut core = core(10, 1_000, 100);
    core.opt_in(A, salt(1), true).unwrap();
    core.opt_in(B, salt(2), true).unwrap();
    core.capture(&event(Some(A), "for a"));
    assert_eq!(messages(&mut core, A), ["for a"]);
    assert!(messages(&mut core, B).is_empty());
}

#[test]
fn agent_wide_lines_are_redacted_and_scoped() {
    let mut core = core(10, 1_000, 100);
    core.learn_peer(A, REMOTE, user("carol")).unwrap();
    core.opt_in(A, salt(1), true).unwrap();
    core.opt_in(B, salt(2), true).unwrap();
    core.capture(&event(None, "carol at 10.0.0.1 failed"));
    core.capture(&event(None, &format!("session {B} dropped")));
    core.capture(&event(None, &format!("peer {REMOTE} dropped")));
    let a = messages(&mut core, A);
    let b = messages(&mut core, B);
    // The line naming B is B's: A never sees it, B does.
    assert_eq!(a.len(), 2, "{a:?}");
    assert_eq!(b.len(), 3, "{b:?}");
    for line in a.iter().chain(&b) {
        assert!(
            !line.contains("carol") && !line.contains("10.0.0.1"),
            "{line}"
        );
    }
    assert!(
        a[1].contains(&REMOTE.to_string()),
        "remote peer cids are kept"
    );
    assert_ne!(a[0], b[0], "each session's batch has its own salt");
}

#[test]
fn a_drain_starts_a_new_salt() {
    let mut core = core(10, 1_000, 100);
    core.opt_in(A, salt(1), true).unwrap();
    core.capture(&event(Some(A), "alice"));
    let first = core.drain(A, salt(2)).unwrap();
    core.capture(&event(Some(A), "alice"));
    let second = core.drain(A, salt(3)).unwrap();
    assert_ne!(first[0].message, second[0].message);
    assert!(!first[0].message.contains("alice"));
}

#[test]
fn a_session_line_naming_another_local_session_redacts_that_cid() {
    let mut core = core(10, 1_000, 100);
    core.opt_in(A, salt(1), true).unwrap();
    core.capture(&event(Some(A), &format!("route {B} via {REMOTE} for {A}")));
    let a = messages(&mut core, A);
    assert_eq!(a.len(), 1, "{a:?}");
    assert!(!a[0].contains(&B.to_string()), "{}", a[0]);
    assert!(a[0].contains("<session:"), "{}", a[0]);
    assert!(
        a[0].contains(&REMOTE.to_string()) && a[0].contains(&A.to_string()),
        "remote peer cids and the session's own cid are kept: {}",
        a[0]
    );
}

//! A window's lines against the real core: its opt-in, its isolation from other sessions, the
//! pacer that keeps it from evicting the agent's own lines, and the same redaction.

use super::*;
use crate::telemetry::config::RingConfig;
use crate::telemetry::entry::{TelemetryEntry, TelemetryLevel};
use crate::telemetry::redact::{KnownValue, RedactionKind, Salt};
use citadel_internal_service_types::BrowserConsoleSource;

const A: u64 = 1_111;
const B: u64 = 2_222;

fn known(kind: RedactionKind, value: &str) -> KnownValue {
    KnownValue {
        kind,
        value: value.into(),
    }
}

/// Alice (A) and Bob (B) signed in on this machine; the bounds are generous unless a test
/// says otherwise.
fn core(ring_entries: usize) -> CollectorCore {
    let mut core = CollectorCore::new(RingConfig::new(ring_entries, 64 * 1024, 2_048).unwrap());
    core.register_session(
        A,
        vec![
            known(RedactionKind::Username, "alice"),
            known(RedactionKind::FullName, "Alice Liddell"),
            known(RedactionKind::Host, "work.avarok.net"),
        ],
    );
    core.register_session(B, vec![known(RedactionKind::Username, "bob")]);
    core
}

fn limits(lines: u32) -> BrowserLimits {
    BrowserLimits::new(lines, 1_000, 50, 4_096).unwrap()
}

fn line(message: &str) -> BrowserConsoleEntry {
    BrowserConsoleEntry {
        source: BrowserConsoleSource::ConsoleError,
        message: message.into(),
        stack: None,
        repeats: 1,
    }
}

fn drained(core: &mut CollectorCore, cid: u64) -> Vec<TelemetryEntry> {
    core.drain(cid, Salt::from_bytes([3; 32])).unwrap()
}

#[test]
fn nothing_is_kept_for_a_session_that_has_not_opted_in() {
    let mut core = core(100);
    core.opt_in(B, Salt::from_bytes([1; 32]), true).unwrap();
    assert_eq!(core.capture_browser(A, 1, &[line("boom")], &limits(10)), 0);
    assert!(
        drained(&mut core, B).is_empty(),
        "A's line reached B's ring"
    );
    assert_eq!(core.capture_browser(77, 1, &[line("boom")], &limits(10)), 0);
}

#[test]
fn a_kept_line_is_the_window_s_session_s_alone() {
    let mut core = core(100);
    core.opt_in(A, Salt::from_bytes([1; 32]), true).unwrap();
    core.opt_in(B, Salt::from_bytes([2; 32]), true).unwrap();
    assert_eq!(core.capture_browser(A, 9, &[line("boom")], &limits(10)), 1);
    let mine = drained(&mut core, A);
    assert_eq!(mine.len(), 1);
    assert_eq!(
        (mine[0].target.as_str(), mine[0].level, mine[0].time_ms),
        ("browser.console", TelemetryLevel::Error, 9)
    );
    assert!(drained(&mut core, B).is_empty());
}

#[test]
fn a_flooding_window_cannot_evict_the_agent_s_own_lines() {
    let mut core = core(10);
    core.opt_in(A, Salt::from_bytes([1; 32]), true).unwrap();
    core.capture(&CapturedEvent {
        time_ms: 1,
        level: TelemetryLevel::Error,
        target: "citadel",
        message: "the agent's own line",
        session_cid: Some(A),
        peer_cid: None,
    });
    let flood: Vec<BrowserConsoleEntry> = (0..50).map(|i| line(&format!("spam {i}"))).collect();
    for request in 0..20 {
        core.capture_browser(A, 2 + request, &flood, &limits(5));
    }
    let kept = drained(&mut core, A);
    assert!(kept.iter().any(|e| e.message == "the agent's own line"));
    assert_eq!(
        kept.iter()
            .filter(|e| e.target == "browser.console")
            .count(),
        5
    );
}

#[test]
fn one_request_carries_at_most_its_bound() {
    let mut core = core(500);
    core.opt_in(A, Salt::from_bytes([1; 32]), true).unwrap();
    let many: Vec<BrowserConsoleEntry> = (0..80).map(|i| line(&format!("l{i}"))).collect();
    assert_eq!(core.capture_browser(A, 1, &many, &limits(1_000)), 50);
}

#[test]
fn extension_lines_never_reach_the_ring() {
    let mut core = core(100);
    core.opt_in(A, Salt::from_bytes([1; 32]), true).unwrap();
    let mut noise = line("Uncaught TypeError: e.closest is not a function");
    noise.stack = Some(
        "at n (chrome-extension://kbfnbcaeplbcioakkpcpgfkobkghlhen/src/js/Grammarly.js:2:1)".into(),
    );
    assert_eq!(core.capture_browser(A, 1, &[noise], &limits(10)), 0);
    assert!(drained(&mut core, A).is_empty());
}

/// Each row: an adversarial console line, and every substring that must not survive.
const ADVERSARIAL: &[(&str, &[&str])] = &[
    (
        "GET https://work.avarok.net/api/v1/files?token=s3cr3tT0k3n&name=report failed",
        &["work.avarok.net", "s3cr3tT0k3n", "/api/v1/files"],
    ),
    (
        "Failed to load resource: http://192.168.1.20:5291/assets/x.js?sig=abc123",
        &["192.168.1.20", "sig=abc123"],
    ),
    (
        concat!(
            "Authorization: Bearer eyJhbGci",
            "OiJIUzI1NiJ9.eyJzdWIiOiIx",
            "MjM0NSJ9.c2lnbmF0dXJl",
            "X2hlcmU"
        ),
        &[
            concat!("eyJhbGci", "OiJIUzI1NiJ9"),
            concat!("c2lnbmF0dXJl", "X2hlcmU"),
        ],
    ),
    (
        "at load (file:///Users/alice/Documents/q3-plan.docx:1:2)",
        &["alice", "/Users/", "q3-plan"],
    ),
    (
        "ENOENT: C:\\Users\\carol\\AppData\\Local\\citadel\\db.sqlite",
        &["carol", "AppData"],
    ),
    (
        "login failed for Alice Liddell <alice.liddell@example.org> password=hunter2",
        &["Alice Liddell", "alice.liddell", "example.org", "hunter2"],
    ),
    (
        concat!(
            "api_key: AKI",
            "A1234ABCD5678EFGH9012 rejected; session 2222 replied"
        ),
        &[concat!("AKI", "A1234ABCD5678EFGH9012"), "2222"],
    ),
    (
        "wss://127.0.0.1:12345/?origin=https%3A%2F%2Fwork.example cookie=sid%3Dabcdef",
        &["127.0.0.1", "sid%3Dabcdef", "work.example"],
    ),
];

#[test]
fn adversarial_lines_lose_every_identifying_part() {
    let mut core = core(100);
    core.opt_in(A, Salt::from_bytes([1; 32]), true).unwrap();
    let mut entries: Vec<BrowserConsoleEntry> = ADVERSARIAL.iter().map(|(l, _)| line(l)).collect();
    entries[3].stack = Some("at x (/home/alice/src/app.ts:3:9)".into());
    let kept = core.capture_browser(A, 1, &entries, &limits(100));
    assert_eq!(kept, ADVERSARIAL.len());
    let stored = drained(&mut core, A);
    for ((input, secrets), entry) in ADVERSARIAL.iter().zip(&stored) {
        for secret in *secrets {
            assert!(
                !entry.message.contains(secret),
                "{secret:?} survived redaction of {input:?}: {:?}",
                entry.message
            );
        }
    }
    assert!(!stored[3].message.contains("/home/alice"));
}

#[test]
fn a_peer_cid_is_kept_as_the_agent_s_own_lines_keep_it() {
    let mut core = core(100);
    core.opt_in(A, Salt::from_bytes([1; 32]), true).unwrap();
    core.capture_browser(A, 1, &[line("peer 9999 did not answer")], &limits(10));
    assert_eq!(drained(&mut core, A)[0].message, "peer 9999 did not answer");
}

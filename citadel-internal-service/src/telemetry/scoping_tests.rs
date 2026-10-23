//! Two sessions on one agent emit interleaved lines through a real tracing subscriber.
//! Each drain holds that session's own lines plus redacted agent-wide lines, never the
//! other session's.

use super::span::{peer_span, session_span};
use super::*;
use citadel_sdk::logging::tracing::{self, log, Instrument};
use tracing_subscriber::layer::SubscriberExt;

const ALICE: u64 = 3_141_592_653;
const BOB: u64 = 2_718_281_828;
const REMOTE_PEER: u64 = 1_618_033_988;

fn collector() -> Arc<TelemetryCollector> {
    // Generous bounds: this test is about scoping, not eviction.
    let collector = TelemetryCollector::new(RingConfig::new(100, 100_000, 1_000).unwrap());
    let name = |value: &str| {
        vec![KnownValue {
            kind: RedactionKind::Username,
            value: value.into(),
        }]
    };
    collector.register_session(ALICE, name("alice"));
    collector.register_session(BOB, name("bob"));
    collector
        .learn_peer(ALICE, REMOTE_PEER, name("carol"))
        .unwrap();
    collector.opt_in(ALICE, true).unwrap();
    collector.opt_in(BOB, true).unwrap();
    collector
}

use std::sync::Arc;

#[tokio::test]
async fn each_session_drains_only_its_own_lines_and_redacted_agent_wide_ones() {
    // The kernel's `log::warn!` lines reach tracing through the log bridge; install it
    // so they are exercised too. Another test may already have installed it.
    let _ = tracing_log::LogTracer::init();
    let collector = collector();
    let subscriber = tracing_subscriber::registry().with(TelemetryLayer::new(collector.clone()));
    let _guard = tracing::subscriber::set_default(subscriber);

    for round in 0..3 {
        async { tracing::warn!("first-only {round}") }
            .instrument(session_span(ALICE))
            .await;
        async { log::warn!(target: "citadel", "second-only {round}") }
            .instrument(session_span(BOB))
            .await;
        tracing::error!("agent-wide {round}: updater could not reach 10.1.2.3");
        async { tracing::warn!("dial to carol {round} failed") }
            .instrument(peer_span(ALICE, REMOTE_PEER))
            .await;
        tracing::warn!("agent-wide about session {BOB}");
        tracing::info!("below the bar {round}");
    }

    let alice = collector.drain(ALICE).unwrap();
    let bob = collector.drain(BOB).unwrap();
    let text = |entries: &[TelemetryEntry]| -> Vec<String> {
        entries.iter().map(|e| e.message.clone()).collect()
    };
    let (a, b) = (text(&alice), text(&bob));

    assert_eq!(
        a.iter().filter(|m| m.starts_with("first-only")).count(),
        3,
        "{a:?}"
    );
    assert_eq!(
        b.iter().filter(|m| m.starts_with("second-only")).count(),
        3,
        "{b:?}"
    );
    assert!(
        a.iter().all(|m| !m.contains("second-only")),
        "bob's line in alice's: {a:?}"
    );
    assert!(
        b.iter().all(|m| !m.contains("first-only")),
        "alice's line in bob's: {b:?}"
    );
    assert!(b.iter().all(|m| !m.contains("carol")), "{b:?}");
    // Agent-wide lines reach both, redacted; the one naming BOB reaches only BOB.
    for drained in [&a, &b] {
        let wide: Vec<_> = drained
            .iter()
            .filter(|m| m.starts_with("agent-wide 0"))
            .collect();
        assert_eq!(wide.len(), 1, "{drained:?}");
        assert!(
            !wide[0].contains("10.1.2.3") && wide[0].contains("<ip:"),
            "{wide:?}"
        );
    }
    assert!(a.iter().all(|m| !m.contains(&BOB.to_string())), "{a:?}");
    assert_eq!(b.iter().filter(|m| m.contains(&BOB.to_string())).count(), 3);
    // The peer line keeps its peer, loses the peer's name, and is tagged with it.
    let peer_lines: Vec<_> = alice
        .iter()
        .filter(|e| e.peer_cid == Some(REMOTE_PEER))
        .collect();
    assert_eq!(peer_lines.len(), 3, "{alice:?}");
    assert!(peer_lines.iter().all(|e| e.message.contains("<user:")));
    assert!(a.iter().chain(&b).all(|m| !m.contains("below the bar")));
    assert!(bob
        .iter()
        .any(|e| e.target == "citadel" && e.message.starts_with("second-only")));
}

#[tokio::test]
async fn a_task_spawned_inside_a_session_keeps_the_session() {
    let collector = collector();
    let subscriber = tracing_subscriber::registry().with(TelemetryLayer::new(collector.clone()));
    let _guard = tracing::subscriber::set_default(subscriber);
    let entered = session_span(ALICE).entered();
    let spawned =
        super::span::spawn_in_current_span(async { tracing::warn!("from a background task") });
    drop(entered);
    // The current-thread test runtime runs the task on this thread, under the default.
    spawned.await.unwrap();
    let alice = collector.drain(ALICE).unwrap();
    let bob = collector.drain(BOB).unwrap();
    assert_eq!(alice.len(), 1, "{alice:?}");
    assert!(
        bob.is_empty(),
        "an untagged task line is agent-wide: {bob:?}"
    );
}

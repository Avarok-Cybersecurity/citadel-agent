//! The uplink against a real collector and tracing layer, with a recording link and a codec
//! whose frames are readable text. The workspace protocol codec is tested where it lives,
//! in the shipped binary.
//!
//! Fakes: `Codec` stands in for the workspace protocol (not a dependency of this crate);
//! `Link` records frames instead of sending them over a C2S channel that needs a server.

use super::*;
use crate::telemetry::span::session_span;
use crate::telemetry::{BrowserLimits, KnownValue, RedactionKind, RingConfig, TelemetryLayer};
use citadel_sdk::logging::tracing::{self, Instrument};
use parking_lot::Mutex;
use tracing_subscriber::layer::SubscriberExt;

const ALICE: u64 = 4_404;
const BOB: u64 = 5_505;

struct Codec;

impl TelemetryCodec for Codec {
    fn consent_query(&self) -> Result<Vec<u8>, String> {
        Ok(b"consent?".to_vec())
    }
    fn upload_frames(&self, entries: Vec<TelemetryEntry>) -> Result<Vec<Vec<u8>>, String> {
        Ok(entries
            .into_iter()
            .map(|e| e.message.into_bytes())
            .collect())
    }
    fn classify(&self, frame: &[u8]) -> ServerSignal {
        match frame {
            b"allowed" => ServerSignal::Consent {
                uploads_allowed: true,
            },
            b"denied" => ServerSignal::Consent {
                uploads_allowed: false,
            },
            b"stored" => ServerSignal::UploadStored,
            b"refused" => ServerSignal::UploadRefused,
            _ => ServerSignal::Unrelated,
        }
    }
}

/// Records every frame per session; `failing` sessions fail every send.
#[derive(Default)]
struct Link {
    sent: Mutex<Vec<(u64, String)>>,
    failing: Vec<u64>,
}

impl SessionLink for Link {
    fn send_to_server(&self, cid: u64, frame: Vec<u8>) -> BoxFuture<'static, Result<(), String>> {
        self.sent
            .lock()
            .push((cid, String::from_utf8(frame).expect("text frames")));
        let failed = self.failing.contains(&cid);
        Box::pin(async move {
            if failed {
                Err("link down".to_owned())
            } else {
                Ok(())
            }
        })
    }
}

fn uplink() -> TelemetryUplink {
    let collector = TelemetryCollector::new(RingConfig::new(100, 100_000, 1_000).unwrap());
    for (cid, name) in [(ALICE, "alice"), (BOB, "bob")] {
        let identity = vec![KnownValue {
            kind: RedactionKind::Username,
            value: name.into(),
        }];
        collector.register_session(cid, identity);
    }
    TelemetryUplink::new(collector, Arc::new(Codec), Duration::from_secs(60)).unwrap()
}

/// Emits one WARN in each session's span through a real layer, then flushes.
async fn warn_each_and_flush(uplink: &TelemetryUplink, link: &Link) {
    let layer = TelemetryLayer::new(uplink.collector().clone());
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));
    async { tracing::warn!("alice could not reach the server") }
        .instrument(session_span(ALICE))
        .await;
    async { tracing::warn!("bob could not reach the server") }
        .instrument(session_span(BOB))
        .await;
    uplink.flush(link).await;
}

fn sent(link: &Link) -> Vec<(u64, String)> {
    link.sent.lock().clone()
}

#[tokio::test]
async fn only_a_session_the_server_allowed_uploads_and_only_its_own_lines() {
    let uplink = uplink();
    assert!(
        uplink.on_server_frame(ALICE, b"allowed"),
        "consent is forwarded"
    );
    let link = Link::default();
    warn_each_and_flush(&uplink, &link).await;
    let sent = sent(&link);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, ALICE);
    assert!(
        sent[0].1.starts_with("<user:") && !sent[0].1.contains("alice"),
        "{}",
        sent[0].1
    );
}

#[tokio::test]
async fn a_session_never_allowed_sends_nothing() {
    let uplink = uplink();
    let link = Link::default();
    warn_each_and_flush(&uplink, &link).await;
    assert!(sent(&link).is_empty(), "{:?}", sent(&link));
}

#[tokio::test]
async fn a_refused_upload_stops_uploads_until_consent_allows_them_again() {
    let uplink = uplink();
    uplink.on_server_frame(ALICE, b"allowed");
    assert!(
        !uplink.on_server_frame(ALICE, b"refused"),
        "the agent's own answer stops here"
    );
    let link = Link::default();
    warn_each_and_flush(&uplink, &link).await;
    assert!(sent(&link).is_empty(), "{:?}", sent(&link));

    uplink.on_server_frame(ALICE, b"allowed");
    warn_each_and_flush(&uplink, &link).await;
    assert_eq!(sent(&link).len(), 1, "{:?}", sent(&link));
}

#[tokio::test]
async fn consent_turned_off_discards_what_was_held() {
    let uplink = uplink();
    uplink.on_server_frame(ALICE, b"allowed");
    let layer = TelemetryLayer::new(uplink.collector().clone());
    {
        let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));
        async { tracing::warn!("held") }
            .instrument(session_span(ALICE))
            .await;
    }
    uplink.on_server_frame(ALICE, b"denied");
    uplink.on_server_frame(ALICE, b"allowed");
    let link = Link::default();
    uplink.flush(&link).await;
    assert!(sent(&link).is_empty(), "{:?}", sent(&link));
}

#[tokio::test]
async fn a_failed_send_drops_the_rest_of_that_drain_and_is_not_retried() {
    let uplink = uplink();
    uplink.on_server_frame(ALICE, b"allowed");
    let link = Link {
        failing: vec![ALICE],
        ..Link::default()
    };
    let layer = TelemetryLayer::new(uplink.collector().clone());
    {
        let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));
        for n in 0..3 {
            async { tracing::warn!("line {n}") }
                .instrument(session_span(ALICE))
                .await;
        }
    }
    uplink.flush(&link).await;
    uplink.flush(&link).await;
    assert_eq!(
        sent(&link).len(),
        1,
        "one attempt, then dropped: {:?}",
        sent(&link)
    );
}

#[test]
fn unrelated_frames_and_stored_answers() {
    let uplink = uplink();
    assert!(uplink.on_server_frame(ALICE, b"workspace data"));
    assert!(!uplink.on_server_frame(ALICE, b"stored"));
    assert!(
        TelemetryUplink::new(uplink.collector().clone(), Arc::new(Codec), Duration::ZERO).is_err()
    );
}

fn console_line(message: &str) -> Vec<BrowserConsoleEntry> {
    vec![BrowserConsoleEntry {
        source: citadel_internal_service_types::BrowserConsoleSource::ConsoleError,
        message: message.into(),
        stack: None,
        repeats: 1,
    }]
}

#[tokio::test]
async fn a_window_s_lines_follow_the_server_s_consent_and_nothing_else() {
    let unconfigured = uplink();
    unconfigured.on_server_frame(ALICE, b"allowed");
    assert!(!unconfigured.takes_browser_console());
    assert_eq!(unconfigured.browser_console(ALICE, &console_line("x")), 0);

    let limits = BrowserLimits::new(10, 1_000, 10, 1_000).unwrap();
    let uplink = uplink().with_browser_console(limits);
    assert!(uplink.takes_browser_console());
    assert_eq!(uplink.browser_console(ALICE, &console_line("early")), 0);
    uplink.on_server_frame(ALICE, b"allowed");
    assert_eq!(
        uplink.browser_console(ALICE, &console_line("alice clicked")),
        1
    );
    assert_eq!(uplink.browser_console(BOB, &console_line("bob clicked")), 0);
    let link = Link::default();
    uplink.flush(&link).await;
    let sent = sent(&link);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0].1.ends_with(" clicked") && !sent[0].1.contains("alice"));
    uplink.on_server_frame(ALICE, b"denied");
    assert_eq!(uplink.browser_console(ALICE, &console_line("late")), 0);
}

//! The core's half of a window's console lines: the session's own opt-in decides, and each
//! admitted line goes through `capture` exactly as the session's own log lines do, so it is
//! redacted, truncated and bounded by the same code.

use super::CollectorCore;
use crate::telemetry::browser::{acceptable, classify, text, BrowserLimits};
use crate::telemetry::entry::CapturedEvent;
use citadel_internal_service_types::BrowserConsoleEntry;

impl CollectorCore {
    /// Takes what `cid`'s window forwarded at `time_ms`; returns how many lines were kept.
    /// Nothing is kept for a session that is unknown or has not opted in.
    pub fn capture_browser(
        &mut self,
        cid: u64,
        time_ms: u64,
        entries: &[BrowserConsoleEntry],
        limits: &BrowserLimits,
    ) -> usize {
        let Some(ring) = self.sessions.get_mut(&cid).and_then(|s| s.ring.as_mut()) else {
            return 0;
        };
        let mut admitted = Vec::new();
        for entry in entries.iter().take(limits.entries_per_request()) {
            if !acceptable(entry, limits) {
                continue;
            }
            if !ring.browser.admit(time_ms, limits) {
                break;
            }
            admitted.push((classify(entry.source), text(entry)));
        }
        for ((target, level), message) in &admitted {
            self.capture(&CapturedEvent {
                time_ms,
                level: *level,
                target,
                message,
                session_cid: Some(cid),
                peer_cid: None,
            });
        }
        admitted.len()
    }
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod tests;

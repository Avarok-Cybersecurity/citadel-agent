//! A window's console lines, taken into its session's ring beside the agent's own.
//!
//! Pure: the time is handed in. The page is not trusted for anything: the target and level
//! come from the line's source, an oversized field drops the line rather than being cut
//! before redaction (a cut can leave the unmatched half of a secret), a line from a browser
//! extension is dropped, and each opted-in session admits a fixed number per window, so a
//! page cannot evict the agent's own lines from the ring by flooding it.

use super::config::TelemetryError;
use super::entry::TelemetryLevel;
use citadel_internal_service_types::{BrowserConsoleEntry, BrowserConsoleSource};

/// Markers of a line raised by a browser extension rather than the app.
const EXTENSION_MARKERS: &[&str] = &[
    "chrome-extension://",
    "moz-extension://",
    "safari-extension://",
    "safari-web-extension://",
    "ms-browser-extension://",
    "grammarly",
];

/// How much of a window's console one session takes. Every bound is the caller's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowserLimits {
    lines_per_window: u32,
    window_ms: u64,
    entries_per_request: usize,
    field_bytes: usize,
}

impl BrowserLimits {
    /// `field_bytes` bounds a line's message and its stack each: a longer one drops the line.
    pub fn new(
        lines_per_window: u32,
        window_ms: u64,
        entries_per_request: usize,
        field_bytes: usize,
    ) -> Result<Self, TelemetryError> {
        if lines_per_window == 0 || window_ms == 0 || entries_per_request == 0 || field_bytes == 0 {
            return Err(TelemetryError::InvalidConfig(
                "every browser console bound must be at least 1",
            ));
        }
        Ok(Self {
            lines_per_window,
            window_ms,
            entries_per_request,
            field_bytes,
        })
    }

    pub fn entries_per_request(&self) -> usize {
        self.entries_per_request
    }
}

/// One session's fixed window: how many lines it admitted since the window opened.
#[derive(Debug, Default)]
pub(super) struct BrowserPacer {
    window_start_ms: Option<u64>,
    admitted: u32,
}

impl BrowserPacer {
    /// Whether one more line fits the window at `now_ms`. A clock that went backwards opens a
    /// new window rather than closing this one for good.
    pub(super) fn admit(&mut self, now_ms: u64, limits: &BrowserLimits) -> bool {
        let expired = self
            .window_start_ms
            .is_none_or(|start| now_ms < start || now_ms.saturating_sub(start) >= limits.window_ms);
        if expired {
            self.window_start_ms = Some(now_ms);
            self.admitted = 0;
        }
        if self.admitted >= limits.lines_per_window {
            return false;
        }
        self.admitted += 1;
        true
    }
}

/// Whether the line may be kept at all: within the field bound, and not an extension's.
pub(super) fn acceptable(entry: &BrowserConsoleEntry, limits: &BrowserLimits) -> bool {
    let stack = entry.stack.as_deref().unwrap_or("");
    entry.message.len() <= limits.field_bytes
        && stack.len() <= limits.field_bytes
        && !from_extension(&entry.message)
        && !from_extension(stack)
}

fn from_extension(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    EXTENSION_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

/// The stored target and level for a source.
pub(super) fn classify(source: BrowserConsoleSource) -> (&'static str, TelemetryLevel) {
    match source {
        BrowserConsoleSource::ConsoleError => ("browser.console", TelemetryLevel::Error),
        BrowserConsoleSource::ConsoleWarn => ("browser.console", TelemetryLevel::Warn),
        BrowserConsoleSource::WindowError => ("browser.error", TelemetryLevel::Error),
        BrowserConsoleSource::UnhandledRejection => ("browser.rejection", TelemetryLevel::Error),
    }
}

/// The line as it is redacted and stored: the message, how often it repeated, then the stack.
pub(super) fn text(entry: &BrowserConsoleEntry) -> String {
    let mut text = entry.message.clone();
    if entry.repeats > 1 {
        text.push_str(&format!(" (repeated {} times)", entry.repeats));
    }
    if let Some(stack) = entry.stack.as_deref().filter(|s| !s.trim().is_empty()) {
        text.push('\n');
        text.push_str(stack);
    }
    text
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod tests;

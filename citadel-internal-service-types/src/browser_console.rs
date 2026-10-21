//! What a window forwards of its own console for the session's opt-in diagnostic logs.
//!
//! The window sends only while the server says the session's uploads are allowed, and only to
//! an agent whose greeting says `browser_console`. The agent decides everything that matters
//! itself: whether the session is opted in, which target and level the line gets, how much of
//! it is kept, and what is redacted. Nothing here is trusted to be small or truthful.

use serde::{Deserialize, Serialize};

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// Where a forwarded line came from. The agent derives the stored target and level from this,
/// so a page cannot name either.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum BrowserConsoleSource {
    ConsoleError,
    ConsoleWarn,
    /// The window's `error` event: an uncaught exception or a script that failed to load.
    WindowError,
    /// The window's `unhandledrejection` event.
    UnhandledRejection,
}

/// One forwarded line, before the agent redacts it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct BrowserConsoleEntry {
    pub source: BrowserConsoleSource,
    pub message: String,
    pub stack: Option<String>,
    /// How many identical lines in a row this one stands for; at least 1.
    pub repeats: u32,
}

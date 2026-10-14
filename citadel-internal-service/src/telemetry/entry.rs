//! What the collector stores and hands out.
//!
//! `TelemetryEntry` is local on purpose: the server's batch type is being defined in
//! citadel-workspace-types, and the upload maps one onto the other field for field.

/// Only WARN and ERROR are captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryLevel {
    Warn,
    Error,
}

/// One redacted line, ready for upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryEntry {
    /// Milliseconds since the Unix epoch.
    pub time_ms: u64,
    pub level: TelemetryLevel,
    pub target: String,
    pub message: String,
    /// The remote peer the line concerns, when it was emitted inside a peer span.
    pub peer_cid: Option<u64>,
}

impl TelemetryEntry {
    /// What the entry counts against the ring's byte bound.
    pub fn byte_len(&self) -> usize {
        self.message.len() + self.target.len()
    }
}

/// An event as the tracing layer saw it, before redaction.
#[derive(Debug, Clone, Copy)]
pub struct CapturedEvent<'a> {
    pub time_ms: u64,
    pub level: TelemetryLevel,
    pub target: &'a str,
    pub message: &'a str,
    /// The session span the event was emitted in; `None` for agent-wide events.
    pub session_cid: Option<u64>,
    pub peer_cid: Option<u64>,
}

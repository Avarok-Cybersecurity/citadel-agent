//! The spans that make a log line attributable to a session.
//!
//! `session{cid}` is entered at the per-session dispatch points (a request acting for a
//! session, a node event for a session, a session's read loops); `peer{cid, peer_cid}`
//! for work on one P2P link. The collector reads these fields; no log line names them.
//!
//! The spans are created at ERROR level so that every subscriber filter that lets any
//! event through also enables them. A WARN line emitted under a span the filter had
//! disabled would lose its session and be treated as agent-wide.

use citadel_sdk::logging::tracing::{self, Span};

pub const SESSION_FIELD: &str = "cid";
pub const PEER_FIELD: &str = "peer_cid";

pub fn session_span(cid: u64) -> Span {
    tracing::error_span!(target: "citadel", "session", cid)
}

/// `Span::none()` when the work acts for no session yet (Connect, Register, ...).
pub fn optional_session_span(cid: Option<u64>) -> Span {
    cid.map_or_else(Span::none, session_span)
}

pub fn peer_span(cid: u64, peer_cid: u64) -> Span {
    tracing::error_span!(target: "citadel", "peer", cid, peer_cid)
}

/// `tokio::spawn` that keeps the caller's span: a spawned task starts with no span, so
/// every line a handler's background task logged used to lose its session.
pub fn spawn_in_current_span<F>(task: F) -> tokio::task::JoinHandle<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(tracing::Instrument::in_current_span(task))
}

/// `tokio::spawn` under `span`, for a task whose session is known where it starts.
pub fn spawn_in_span<F>(span: Span, task: F) -> tokio::task::JoinHandle<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(tracing::Instrument::instrument(task, span))
}

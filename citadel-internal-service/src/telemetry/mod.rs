//! Opt-in telemetry: WARN and ERROR lines, scoped to the session that emitted them,
//! redacted, held in a bounded in-memory ring per opted-in session until drained.
//!
//! - `span`: the `session{cid}` / `peer{cid, peer_cid}` spans the kernel enters.
//! - `layer`: the tracing layer that reads them.
//! - `core`: the pure collector; `collector`: its lock-and-randomness shell.
//! - `redact`: pure redaction rules.
//! - `browser`: a window's console lines, taken into the same rings.

pub mod browser;
pub mod collector;
pub mod config;
mod core;
pub mod entry;
pub mod layer;
pub mod redact;
pub mod span;
pub mod uplink;

pub use browser::BrowserLimits;
pub use collector::TelemetryCollector;
pub use config::{RingConfig, TelemetryError};
pub use entry::{TelemetryEntry, TelemetryLevel};
pub use layer::TelemetryLayer;
pub use redact::{KnownValue, RedactionKind};
pub use uplink::{ServerSignal, SessionLink, TelemetryCodec, TelemetryUplink};

#[cfg(test)]
mod scoping_tests;

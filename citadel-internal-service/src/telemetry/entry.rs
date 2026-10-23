//! What the collector stores and hands out.
//!
//! `TelemetryEntry` is local on purpose: the server's batch type is being defined in
//! citadel-workspace-types, and the upload maps one onto the other field for field.

use citadel_sdk::prelude::{NatClass, P2pPath, PathAttemptOutcome};

/// WARN and ERROR lines are captured, and metrics (`TelemetryEntry::metric`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryLevel {
    Metric,
    Warn,
    Error,
}

/// A structured measurement, carried by an entry at `TelemetryLevel::Metric`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelemetryMetric {
    PathReport(PathReport),
}

impl TelemetryMetric {
    /// What the metric counts against the ring's byte bound: an estimate of its uploaded size.
    pub fn byte_len(&self) -> usize {
        const FIXED: usize = 192;
        const PER_STAGE: usize = 64;
        match self {
            TelemetryMetric::PathReport(report) => FIXED + PER_STAGE * report.stages.len(),
        }
    }
}

/// How one P2P path-upgrade attempt ended: the SDK's `PathAttemptOutcome`, which only the SDK
/// can construct, copied into a value the rest of the pipeline (and its tests) can hold. The
/// SDK's plan, stage and failure enums are open-ended, so they travel as their variant names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathReport {
    pub attempt: u32,
    pub plan: String,
    pub path: P2pPath,
    pub relayed_both: bool,
    pub time_to_connect_ms: Option<u64>,
    pub stages: Vec<StageReport>,
    pub local_nat: NatClass,
    pub peer_nat: NatClass,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageReport {
    pub stage: String,
    /// `None` when the stage connected.
    pub failure: Option<String>,
    pub elapsed_ms: u64,
}

impl From<&PathAttemptOutcome> for PathReport {
    fn from(outcome: &PathAttemptOutcome) -> Self {
        Self {
            attempt: outcome.attempt,
            plan: format!("{:?}", outcome.plan),
            path: outcome.path,
            relayed_both: outcome.relayed_both,
            time_to_connect_ms: outcome.time_to_connect().map(|d| d.as_millis() as u64),
            stages: outcome
                .stages
                .iter()
                .map(|s| StageReport {
                    stage: format!("{:?}", s.stage),
                    failure: s.result.err().map(|f| format!("{f:?}")),
                    elapsed_ms: s.elapsed.as_millis() as u64,
                })
                .collect(),
            local_nat: outcome.local_nat,
            peer_nat: outcome.peer_nat,
        }
    }
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
    /// Exactly when `level` is `Metric`: what was measured.
    pub metric: Option<TelemetryMetric>,
}

impl TelemetryEntry {
    /// What the entry counts against the ring's byte bound.
    pub fn byte_len(&self) -> usize {
        self.message.len() + self.target.len() + self.metric.as_ref().map_or(0, |m| m.byte_len())
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

//! The core's half of metrics: a session's own measurement goes to that session's ring only,
//! and only once it has opted in, bounded like its log lines. A metric carries no free text
//! (its names are the protocol's), so there is nothing in it to redact; the peer's CID is kept,
//! as it is on log lines, so issues between peers can be traced.

use super::CollectorCore;
use crate::telemetry::entry::{TelemetryEntry, TelemetryLevel, TelemetryMetric};

/// The target metric entries are filed under.
pub const METRIC_TARGET: &str = "citadel::metric";

impl CollectorCore {
    /// Keeps `metric`, measured at `time_ms` on `cid`'s connection to `peer_cid`; returns whether
    /// it was kept (it is not for a session that is unknown or has not opted in).
    pub fn capture_metric(
        &mut self,
        cid: u64,
        time_ms: u64,
        peer_cid: u64,
        metric: TelemetryMetric,
    ) -> bool {
        let config = self.config;
        let Some(ring) = self.sessions.get_mut(&cid).and_then(|s| s.ring.as_mut()) else {
            return false;
        };
        let entry = TelemetryEntry {
            time_ms,
            level: TelemetryLevel::Metric,
            target: METRIC_TARGET.to_owned(),
            message: summary(&metric),
            peer_cid: Some(peer_cid),
            metric: Some(metric),
        };
        ring.push(entry, &config);
        true
    }
}

/// The line an admin reads beside the metric.
fn summary(metric: &TelemetryMetric) -> String {
    match metric {
        TelemetryMetric::PathReport(report) => format!(
            "path attempt {}: {} plan, attached {:?}",
            report.attempt, report.plan, report.path
        ),
    }
}

#[cfg(test)]
#[path = "metric_tests.rs"]
mod tests;

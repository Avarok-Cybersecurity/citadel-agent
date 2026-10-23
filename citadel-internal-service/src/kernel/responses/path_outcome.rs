//! The SDK's report of each P2P path-upgrade attempt (`NodeResult::PeerPathOutcome`), taken
//! into the session's opt-in telemetry as a path report. Nothing is kept for a session that has
//! not opted in, and nothing about the connection changes.

use crate::kernel::CitadelWorkspaceService;
use crate::telemetry::{PathReport, TelemetryMetric};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::logging::debug;
use citadel_sdk::prelude::{PeerPathOutcome, Ratchet};

pub(super) fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    outcome: &PeerPathOutcome,
) {
    let Some(uplink) = this.telemetry.as_ref() else {
        return;
    };
    let peer_cid = outcome.outcome.peer_cid;
    let report = PathReport::from(&outcome.outcome);
    let kept = uplink.collector().capture_metric(
        outcome.session_cid,
        peer_cid,
        TelemetryMetric::PathReport(report),
    );
    debug!(target: "citadel", "[PeerPath] {}: attempt {} to {peer_cid} ended on {:?}; telemetry kept it: {kept}", outcome.session_cid, outcome.outcome.attempt, outcome.outcome.path);
}

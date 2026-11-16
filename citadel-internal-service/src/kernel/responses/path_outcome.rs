//! The SDK's report of each P2P path-upgrade attempt (`NodeResult::PeerPathOutcome`), taken
//! into the session's opt-in telemetry as a path report. Nothing is kept for a session that has
//! not opted in, and nothing about the connection changes.
//!
//! Every attempt is also logged with both NAT classes and the plan: whether a pair could get a
//! direct path (and so a UDP channel, which a call needs) is decided by those classes, and an
//! integration run that cannot show them cannot say why its calls had no UDP.

use crate::kernel::CitadelWorkspaceService;
use crate::telemetry::{PathReport, TelemetryMetric};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::logging::{debug, info};
use citadel_sdk::prelude::{PeerPathOutcome, Ratchet};

pub(super) fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    outcome: &PeerPathOutcome,
) {
    let attempt = &outcome.outcome;
    info!(target: "citadel", "[PeerPath] {} -> {}: attempt {} planned {:?}, attached {:?}; local NAT {:?}, peer NAT {:?}", outcome.session_cid, attempt.peer_cid, attempt.attempt, attempt.plan, attempt.path, attempt.local_nat, attempt.peer_nat);
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

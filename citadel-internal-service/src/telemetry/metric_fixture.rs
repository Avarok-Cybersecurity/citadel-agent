//! A path report for tests: one upgrade attempt that fell back from Direct to TURN.

use crate::telemetry::entry::{PathReport, StageReport, TelemetryMetric};
use citadel_sdk::prelude::{IpTranslationClass, NatClass, P2pPath, PortTranslationClass};

fn nat() -> NatClass {
    NatClass {
        ip_translation: IpTranslationClass::Constant,
        port_translation: PortTranslationClass::Identity,
        ipv6_enabled: false,
    }
}

pub(crate) fn path_report(attempt: u32) -> TelemetryMetric {
    TelemetryMetric::PathReport(PathReport {
        attempt,
        plan: "DirectThenTurn".into(),
        path: P2pPath::Turn,
        relayed_both: true,
        time_to_connect_ms: Some(900),
        stages: vec![StageReport {
            stage: "Direct".into(),
            failure: Some("PunchFailed".into()),
            elapsed_ms: 400,
        }],
        local_nat: nat(),
        peer_nat: nat(),
    })
}

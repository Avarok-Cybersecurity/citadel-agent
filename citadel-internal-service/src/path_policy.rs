//! The path policy the agent repository's own development servers run (`citadel-service-bin`'s
//! empty server and the websocket example). A Citadel server requires one and the SDK has no
//! default, so these binaries name theirs here: every path in the protocol's order, every
//! traffic class on each, and the 30 s attempts the campaign has always used. The workspace
//! server takes its policy from kernel.toml or its host instead.

use citadel_sdk::prelude::{PathClasses, PathKind, PathPolicy, PathPolicyError, PathRule};
use std::time::Duration;

/// How long each attempt at a path may take on a development server.
pub const DEV_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(30);

pub fn every_path_policy() -> Result<PathPolicy, PathPolicyError> {
    let rules = PathKind::ALL
        .into_iter()
        .map(|kind| PathRule {
            kind,
            enabled: true,
            attempt_timeout: DEV_ATTEMPT_TIMEOUT,
            cost_weight: 1,
            classes: PathClasses::ALL,
        })
        .collect();
    PathPolicy::new(rules, PathClasses::NONE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_development_policy_is_valid_and_carries_everything_everywhere() {
        let policy = every_path_policy().expect("valid");
        assert_eq!(
            policy.rules().iter().map(|r| r.kind).collect::<Vec<_>>(),
            PathKind::ALL.to_vec()
        );
        assert!(policy
            .rules()
            .iter()
            .all(|r| r.enabled && r.classes == PathClasses::ALL));
    }
}

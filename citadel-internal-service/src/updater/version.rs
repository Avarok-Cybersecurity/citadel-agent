//! Release tags and the versions they name (pure).

use semver::Version;

/// Agent releases are tagged `agent-vMAJOR.MINOR.PATCH`; every other tag in the repository is
/// something else's.
pub const TAG_PREFIX: &str = "agent-v";

/// The version an agent release tag names: `agent-v0.8.8` is 0.8.8. `None` for any other tag,
/// and for a prerelease or build-tagged version, which the updater never installs.
pub fn version_of_tag(tag: &str) -> Option<Version> {
    let version = Version::parse(tag.strip_prefix(TAG_PREFIX)?).ok()?;
    (version.pre.is_empty() && version.build.is_empty()).then_some(version)
}

/// The running agent's version, as `CARGO_PKG_VERSION` spells it.
pub fn running(text: &str) -> Result<Version, String> {
    Version::parse(text).map_err(|e| format!("the running version {text:?} is not semver: {e}"))
}

/// Whether `candidate` is an upgrade from `current`. Equal is not: reinstalling the same
/// version gains nothing and restarts the agent for it.
pub fn is_upgrade(current: &Version, candidate: &Version) -> bool {
    candidate > current
}

/// What `<binary> --version` prints for `version`: structopt's `name version`.
pub fn version_line(version: &Version) -> String {
    format!("citadel-agent {version}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn an_agent_tag_names_its_version() {
        assert_eq!(version_of_tag("agent-v0.8.8"), Some(v("0.8.8")));
        assert_eq!(version_of_tag("agent-v10.0.1"), Some(v("10.0.1")));
    }

    #[test]
    fn other_tags_and_unstable_versions_name_nothing() {
        for tag in [
            "v0.8.8",
            "agent-0.8.8",
            "agent-v0.8",
            "agent-v0.8.8-rc.1",
            "agent-v0.8.8+build",
            "ui-v1.0.0",
            "",
        ] {
            assert_eq!(version_of_tag(tag), None, "{tag}");
        }
    }

    #[test]
    fn only_a_higher_version_is_an_upgrade() {
        assert!(is_upgrade(&v("0.8.7"), &v("0.8.8")));
        assert!(is_upgrade(&v("0.8.9"), &v("0.10.0")));
        assert!(!is_upgrade(&v("0.8.8"), &v("0.8.8")));
        assert!(!is_upgrade(&v("0.8.8"), &v("0.8.7")));
        assert!(!is_upgrade(&v("1.0.0"), &v("0.99.99")));
    }

    #[test]
    fn the_version_line_is_what_structopt_prints() {
        assert_eq!(version_line(&v("0.8.8")), "citadel-agent 0.8.8");
    }

    #[test]
    fn a_running_version_that_is_not_semver_is_an_error() {
        assert!(running("0.8").is_err());
        assert_eq!(running("0.8.8").unwrap(), v("0.8.8"));
    }
}

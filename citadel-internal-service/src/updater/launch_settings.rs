//! The launch settings a release requires (pure): its asset `citadel-agent-launch.json`,
//! `{"v":1,"settings":{"--flag":"value"}}`, signed like every other asset (`.mldsa.sig`).
//!
//! A tarball's agent is started by hand with a command its operator wrote. When a release adds
//! a setting the agent refuses to start without, that command lacks it, and the staged version
//! would refuse to launch (launch_check.rs). The asset carries every required setting with the
//! value the packaged launchers ship, so the updater can add the ones the command lacks.
//!
//! Only flags the command does not give are added: a value the operator chose always wins, and
//! one given by environment variable still wins too, because every such setting resolves the
//! variable over the flag.

use serde::Deserialize;
use std::collections::BTreeMap;

pub const ASSET_NAME: &str = "citadel-agent-launch.json";
/// The one format this agent reads; any other is refused rather than guessed at.
pub const FORMAT: u64 = 1;
/// Far above a few settings, and small enough that a hostile response cannot fill memory.
pub const MAX_BYTES: u64 = 16 * 1024;
const MAX_VALUE_LEN: usize = 1024;
/// What an updating agent asks a staged version (launch_check.rs, which is unix-only; the flag
/// is here so this module is not).
pub const CHECK_LAUNCH_FLAG: &str = "--check-launch";
/// Never supplied: an agent started with it checks its settings and exits, so every restart
/// would end at once.
const NEVER_SUPPLIED: [&str; 1] = [CHECK_LAUNCH_FLAG];

/// What the release says this agent must be started with, verified and well-formed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSettings(BTreeMap<String, String>);

#[derive(Deserialize)]
struct Format {
    v: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V1 {
    #[allow(dead_code)]
    v: u64,
    settings: BTreeMap<String, String>,
}

/// Why a release without the asset, or without its signature, is refused.
pub fn missing(what: &str) -> String {
    format!("the release has no {what}, so this agent cannot know what the new version must be started with; nothing was installed")
}

/// The asset's bytes, already verified against the release key.
pub fn parse(bytes: &[u8]) -> Result<LaunchSettings, String> {
    let refused = |why: String| format!("{ASSET_NAME}: {why}; nothing was installed");
    let format: Format =
        serde_json::from_slice(bytes).map_err(|e| refused(format!("unreadable ({e})")))?;
    if format.v != FORMAT {
        return Err(refused(format!(
            "format {} is not the {FORMAT} this agent reads; download the new version by hand",
            format.v
        )));
    }
    let v1: V1 = serde_json::from_slice(bytes).map_err(|e| refused(format!("malformed ({e})")))?;
    for (flag, value) in &v1.settings {
        well_formed(flag, value).map_err(refused)?;
    }
    Ok(LaunchSettings(v1.settings))
}

fn well_formed(flag: &str, value: &str) -> Result<(), String> {
    let name = flag.strip_prefix("--").unwrap_or_default();
    let shaped = name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !shaped {
        return Err(format!("{flag:?} is not a long flag"));
    }
    if NEVER_SUPPLIED.contains(&flag) {
        return Err(format!("{flag} is never a launch setting"));
    }
    if value.is_empty() || value.len() > MAX_VALUE_LEN || value.chars().any(char::is_control) {
        return Err(format!(
            "the value for {flag} is empty, too long or not printable"
        ));
    }
    Ok(())
}

impl LaunchSettings {
    /// `args` with every setting it does not give appended, in the asset's order. A flag counts
    /// as given as `--flag value` or `--flag=value`; given flags and their values are untouched.
    pub fn merged(&self, args: &[String]) -> Vec<String> {
        let given = |flag: &str| {
            args.iter().any(|arg| {
                arg == flag
                    || arg
                        .strip_prefix(flag)
                        .is_some_and(|rest| rest.starts_with('='))
            })
        };
        let mut merged = args.to_vec();
        for (flag, value) in &self.0 {
            if !given(flag) {
                merged.push(flag.clone());
                merged.push(value.clone());
            }
        }
        merged
    }
}

#[cfg(test)]
#[path = "launch_settings_tests.rs"]
mod tests;

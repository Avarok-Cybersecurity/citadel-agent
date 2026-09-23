//! The STUN servers the agent hands the SDK for NAT identification.
//!
//! There is no default. The SDK falls back to a built-in list when none is
//! given, which means every agent quietly reports its public address to
//! whichever third parties that list names. The operator chooses them instead,
//! and the agent refuses to start until they have.

use citadel_sdk::prelude::{NodeBuilder, Ratchet};
use std::net::Ipv6Addr;

/// The environment variable that overrides [`STUN_SERVERS_FLAG`].
pub const STUN_SERVERS_ENV: &str = "INTERNAL_SERVICE_STUN_SERVERS";
/// The command-line flag the binaries accept the list on.
pub const STUN_SERVERS_FLAG: &str = "--stun-servers";

/// Exactly how many servers the SDK accepts. `NodeBuilder::build` refuses any
/// other count ("There must be exactly 3 specified STUN servers"), because NAT
/// classification compares the address each of three servers reports. Checked
/// here so a wrong count names the flag instead of surfacing from the SDK.
pub const STUN_SERVER_COUNT: usize = 3;

/// Longest `host:port` accepted: a maximal DNS name plus `:65535`.
const MAX_ENTRY_LEN: usize = 253 + 6;
const MAX_DNS_NAME_LEN: usize = 253;
const MAX_DNS_LABEL_LEN: usize = 63;

/// A validated list of exactly [`STUN_SERVER_COUNT`] `host:port` entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StunServers(Vec<String>);

/// Where a [`StunServers`] list is delivered. Implemented for the SDK's
/// `NodeBuilder`; the seam exists because the builder offers no way to read
/// the list back, so a test can only observe delivery through it.
pub trait StunServerSink {
    fn set_stun_servers(&mut self, servers: Vec<String>);
}

impl<R: Ratchet> StunServerSink for NodeBuilder<R> {
    fn set_stun_servers(&mut self, servers: Vec<String>) {
        self.with_stun_servers(servers);
    }
}

impl StunServers {
    /// Resolve the list from env + CLI. Env wins, as for the origin allowlist
    /// and the backend, so a docker operator can change it without rebuilding.
    /// Empty strings are unset: a blank `.env` entry arrives as `Some("")`.
    pub fn resolve(env_spec: Option<&str>, cli_spec: Option<&str>) -> Result<Self, String> {
        let spec = env_spec
            .filter(|s| !s.is_empty())
            .or(cli_spec.filter(|s| !s.is_empty()));

        let Some(spec) = spec else {
            return Err(format!(
                "no STUN servers configured. Pass {STUN_SERVERS_FLAG} (or set {STUN_SERVERS_ENV}) \
                 to {STUN_SERVER_COUNT} comma-separated host:port entries, e.g. \
                 \"stun.cloudflare.com:3478,stun1.l.google.com:19302,stun4.l.google.com:19302\"."
            ));
        };

        Self::parse(spec).map_err(|why| format!("invalid {STUN_SERVERS_FLAG}: {why}"))
    }

    /// Parse a comma-separated `host:port` list. Entries are trimmed; each must
    /// be a DNS name, IPv4 address or bracketed IPv6 address with a non-zero
    /// port, and carry no scheme, path or credentials.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let entries = spec
            .split(',')
            .map(|entry| validate_entry(entry.trim()))
            .collect::<Result<Vec<_>, _>>()?;

        if entries.len() != STUN_SERVER_COUNT {
            return Err(format!(
                "expected exactly {STUN_SERVER_COUNT} STUN servers, got {}",
                entries.len()
            ));
        }

        Ok(Self(entries))
    }

    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    /// Hand the list to `builder`.
    pub fn apply<'a, B: StunServerSink>(&self, builder: &'a mut B) -> &'a mut B {
        builder.set_stun_servers(self.0.clone());
        builder
    }
}

fn validate_entry(entry: &str) -> Result<String, String> {
    if entry.is_empty() {
        return Err("empty entry".into());
    }
    if entry.len() > MAX_ENTRY_LEN {
        return Err(format!(
            "entry longer than {MAX_ENTRY_LEN} characters: {entry:?}"
        ));
    }
    if has_scheme(entry) {
        return Err(format!(
            "{entry:?} has a scheme; give host:port only, e.g. \"stun.cloudflare.com:3478\""
        ));
    }
    if entry.contains(['/', '?', '#', '@']) || entry.chars().any(char::is_whitespace) {
        return Err(format!("{entry:?} is not a bare host:port"));
    }

    let (host, port) = entry
        .rsplit_once(':')
        .ok_or_else(|| format!("{entry:?} has no port; expected host:port"))?;
    match port.parse::<u16>() {
        Ok(0) | Err(_) => return Err(format!("{entry:?} has an invalid port {port:?}")),
        Ok(_) => {}
    }
    if !is_valid_host(host) {
        return Err(format!("{entry:?} has an invalid host {host:?}"));
    }

    Ok(entry.to_string())
}

fn has_scheme(entry: &str) -> bool {
    let lower = entry.to_ascii_lowercase();
    lower.contains("://")
        || ["stun:", "stuns:", "turn:", "turns:"]
            .iter()
            .any(|scheme| lower.starts_with(scheme))
}

fn is_valid_host(host: &str) -> bool {
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return inner.parse::<Ipv6Addr>().is_ok();
    }
    !host.is_empty()
        && host.len() <= MAX_DNS_NAME_LEN
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= MAX_DNS_LABEL_LEN
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

#[cfg(test)]
#[path = "stun_tests.rs"]
mod tests;

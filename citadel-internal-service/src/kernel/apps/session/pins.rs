//! How the agent finds an app's server (`citadel-session-abi.md` §4). Every server is
//! discovered by its `/.well-known/citadel` and dialled at the `citadel.wss` it names, except a
//! pinned one: a server without HTTPS, pinned to the operator's fingerprint until its challenge
//! proves the key, as `RegisterIdentity`'s `Pinned` discovery does, and dialled at the address
//! its pin carries. The canonical domain stays the account's: its username, the domain its
//! challenge names, the manifest's `servers` entry. Explicit: the shipped agent pins nothing.
//!
//! A held account is signed in to again only while the configuration still says what it was
//! registered with: the SDK dials the address an account registered at, so a pin removed or
//! changed since is refused rather than dialled under the old one. Pure.

use crate::kernel::account_server;
use crate::kernel::identity::pin::{normalise_fingerprint, Pin};
use crate::kernel::identity::plan::Discovery;
use crate::kernel::server_address::dial_form;
use citadel_identity::canonical_domain;
use std::collections::HashMap;
use std::fmt;

/// One pin as configured, before it is validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinEntry {
    pub domain: String,
    pub fingerprint: String,
    pub dial: String,
}

/// A validated pin: only `AppSessionConfig::new` makes one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedServer {
    fingerprint: String,
    dial: String,
}

impl PinnedServer {
    /// As `citadel_identity::fingerprint` writes it.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Where the socket goes, in `server_address::dial_form`'s form.
    pub fn dial(&self) -> &str {
        &self.dial
    }
}

/// Why entry number `entry` (from 0, in the order given) is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinError {
    pub entry: usize,
    pub domain: String,
    pub why: String,
}

impl fmt::Display for PinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the pin for {:?}: {}", self.domain, self.why)
    }
}

impl std::error::Error for PinError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSessionConfig {
    /// Canonical domain → its pin.
    pinned_servers: HashMap<String, PinnedServer>,
}

fn validated(entry: &PinEntry) -> Result<PinnedServer, String> {
    let canonical = canonical_domain(&entry.domain).map_err(|err| err.to_string())?;
    if canonical.as_str() != entry.domain {
        return Err(format!(
            "the domain is not canonical; write it as {:?}",
            canonical.as_str()
        ));
    }
    let normalised = normalise_fingerprint(&entry.fingerprint)?;
    if normalised != entry.fingerprint {
        return Err(format!(
            "the fingerprint is not as `citadel_identity::fingerprint` writes it; write it as {normalised:?}"
        ));
    }
    let dial = dial_form(&entry.dial).map_err(|err| format!("the dial address: {err}"))?;
    Ok(PinnedServer {
        fingerprint: normalised,
        dial,
    })
}

impl AppSessionConfig {
    /// Every entry validated, and no domain pinned twice.
    pub fn new(entries: impl IntoIterator<Item = PinEntry>) -> Result<Self, PinError> {
        let mut pinned_servers = HashMap::new();
        for (index, entry) in entries.into_iter().enumerate() {
            let refused = |why: String| PinError {
                entry: index,
                domain: entry.domain.clone(),
                why,
            };
            let pin = validated(&entry).map_err(refused)?;
            if pinned_servers.contains_key(&entry.domain) {
                return Err(refused("the domain is pinned more than once".to_string()));
            }
            pinned_servers.insert(entry.domain, pin);
        }
        Ok(Self { pinned_servers })
    }

    pub fn pinned(&self, domain: &str) -> Option<&PinnedServer> {
        self.pinned_servers.get(domain)
    }

    pub fn pins(&self) -> impl Iterator<Item = (&str, &PinnedServer)> {
        self.pinned_servers.iter().map(|(d, p)| (d.as_str(), p))
    }

    /// How a first registration on `domain` (canonical) finds its server.
    pub(crate) fn discovery(&self, domain: &str) -> Discovery {
        match self.pinned(domain) {
            Some(pin) => Discovery::Pinned {
                server_fp: pin.fingerprint.clone(),
                dial: pin.dial.clone(),
            },
            None => Discovery::WellKnown,
        }
    }

    /// Whether a held account on `domain`, registered at `held` (as `server_host` recorded it)
    /// under `record`, may be signed in to again: only as the configuration would register it.
    pub(crate) fn sign_in_again(
        &self,
        domain: &str,
        held: &str,
        record: &Pin,
    ) -> Result<(), String> {
        match (self.pinned(domain), record.discovered()) {
            (None, true) => Ok(()),
            (None, false) => Err(format!(
                "the account was registered through a pin for {domain}, at {held}, and the \
                 agent's configuration no longer pins {domain}; nothing was dialled"
            )),
            (Some(pin), true) => Err(format!(
                "the account was registered through {domain}'s /.well-known/citadel, and the \
                 agent's configuration now pins {domain} to {}; nothing was dialled",
                pin.dial
            )),
            (Some(pin), false) => {
                let dial = account_server::canonical(&pin.dial)?;
                if dial != held || pin.fingerprint != record.server_fp {
                    return Err(format!(
                        "the account was registered through a pin for {domain} at {held} with \
                         fingerprint {}, and the agent's configuration now pins it at {} with \
                         fingerprint {}; nothing was dialled",
                        record.server_fp, pin.dial, pin.fingerprint
                    ));
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
#[path = "pins_tests.rs"]
mod tests;

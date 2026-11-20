//! Sealing every object an account stored again, under the root its new recovery phrase
//! encodes. Nothing can be re-wrapped: the SDK derives each object's layer keys from the
//! account's key itself (`citadel_crypt::revfs_seal`), so each object is read under the key it
//! is sealed under and written again under the new one.
//!
//! A store overwrites in place and is not atomic at the far end: a store that dies half way
//! leaves the object unreadable. So no object's only readable copy is ever overwritten:
//!
//! 1. read the object under its key; write it, sealed under the new key, to its staging path;
//! 2. record the entry as `Staged` (the staged copy is complete and readable);
//! 3. write it over its own path under the new key, delete the staged copy, and record the
//!    entry as `Settled` under the new key.
//!
//! A pass that stops anywhere (a failure, a restart) leaves every object readable under a root
//! the account still holds, and the next pass resumes from the index. A retired root is dropped
//! only when no entry names it (node.rs). Pure: every effect is a [`RekeyPorts`] call.

use super::index::{IndexEntry, Location, RevfsIndex, Stage};
use super::roots::KeyId;
use async_trait::async_trait;
use citadel_sdk::prelude::RevfsKey;
use std::path::{Path, PathBuf};

const STAGING_SUFFIX: &str = "~citadel-rekey";

/// Where an object's re-keyed copy is written before the object itself is.
pub(crate) fn staging(path: &str) -> String {
    format!("{path}{STAGING_SUFFIX}")
}

/// What a pass can do. Each call either happened or did not; a store that fails may have
/// damaged what was at its path.
#[async_trait]
pub(crate) trait RekeyPorts: Send + Sync {
    /// Whether a pass can reach `location` now (a peer must be connected).
    fn reachable(&self, location: Location) -> bool;
    /// Reads the object at `path`, opened under `key`, into a local file.
    async fn pull(&self, location: Location, path: &str, key: &RevfsKey)
        -> Result<PathBuf, String>;
    async fn push(
        &self,
        location: Location,
        path: &str,
        level: u8,
        local: &Path,
        key: &RevfsKey,
    ) -> Result<(), String>;
    async fn delete(&self, location: Location, path: &str) -> Result<(), String>;
    /// Removes a local file a pull wrote: it holds the object's plaintext.
    async fn discard(&self, local: &Path);
    /// Records `entry` in place of the index's entry for the same object.
    async fn record(&self, entry: &IndexEntry) -> Result<(), String>;
}

/// The account's keys: the current root's and the retired roots'.
pub(crate) struct Keys {
    pub current: RevfsKey,
    pub retired: Vec<RevfsKey>,
}

impl Keys {
    fn find(&self, id: KeyId) -> Option<&RevfsKey> {
        std::iter::once(&self.current)
            .chain(self.retired.iter())
            .find(|key| key.id() == id)
    }
}

/// What a pass did. It is finished when nothing is left waiting or failed.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Report {
    pub rekeyed: usize,
    /// Places a pass could not reach; their objects keep their keys for the next pass.
    pub unreachable: Vec<Location>,
    /// `(location, path, why)` of each object the pass could not re-key.
    pub failed: Vec<(Location, String, String)>,
}

impl Report {
    pub(crate) fn finished(&self) -> bool {
        self.unreachable.is_empty() && self.failed.is_empty()
    }
}

/// The entries a pass has to visit: those not settled under the current key.
pub(crate) fn pending(index: &RevfsIndex, current: KeyId) -> Vec<IndexEntry> {
    index
        .entries
        .iter()
        .filter(|entry| entry.stage != Stage::Settled || entry.key_id != current)
        .cloned()
        .collect()
}

pub(crate) const UNKNOWN_KEY: &str = "sealed under a key this device no longer holds";

/// Re-keys each entry it can reach, in order; one object's failure does not stop the others.
pub(crate) async fn run(ports: &dyn RekeyPorts, keys: &Keys, entries: Vec<IndexEntry>) -> Report {
    let mut report = Report::default();
    for entry in entries {
        let location = entry.location;
        if !ports.reachable(location) {
            if !report.unreachable.contains(&location) {
                report.unreachable.push(location);
            }
            continue;
        }
        let path = entry.path.clone();
        match rekey_one(ports, keys, entry).await {
            Ok(()) => report.rekeyed += 1,
            Err(why) => report.failed.push((location, path, why)),
        }
    }
    report
}

async fn rekey_one(
    ports: &dyn RekeyPorts,
    keys: &Keys,
    mut entry: IndexEntry,
) -> Result<(), String> {
    let current = &keys.current;
    let staged_path = staging(&entry.path);
    // At most: recover a staged entry to `Settled` under a retired key, then re-key it.
    for _ in 0..2 {
        let local = match entry.stage {
            Stage::Settled if entry.key_id == current.id() => return Ok(()),
            Stage::Settled => {
                let old = keys.find(entry.key_id).ok_or(UNKNOWN_KEY)?;
                let local = ports.pull(entry.location, &entry.path, old).await?;
                let staged = stage(ports, current, &mut entry, &local, &staged_path).await;
                if let Err(why) = staged {
                    ports.discard(&local).await;
                    return Err(why);
                }
                local
            }
            Stage::Staged { key_id } => {
                let staged_key = keys.find(key_id).ok_or(UNKNOWN_KEY)?;
                match ports.pull(entry.location, &staged_path, staged_key).await {
                    Ok(local) => local,
                    // The staged copy is deleted only after the object itself was written, so
                    // the object opens under the staged copy's key.
                    Err(why) => {
                        let local = ports
                            .pull(entry.location, &entry.path, staged_key)
                            .await
                            .map_err(|_| why)?;
                        ports.discard(&local).await;
                        entry.key_id = key_id;
                        entry.stage = Stage::Settled;
                        ports.record(&entry).await?;
                        continue;
                    }
                }
            }
        };
        let finished = finish(ports, current, &mut entry, &local, &staged_path).await;
        ports.discard(&local).await;
        return finished;
    }
    Err("the object's re-key did not settle".to_string())
}

/// Steps 1 and 2: a complete copy under the new key at the staging path, then the record.
async fn stage(
    ports: &dyn RekeyPorts,
    current: &RevfsKey,
    entry: &mut IndexEntry,
    local: &Path,
    staged_path: &str,
) -> Result<(), String> {
    ports
        .push(entry.location, staged_path, entry.level, local, current)
        .await?;
    entry.stage = Stage::Staged {
        key_id: current.id(),
    };
    ports.record(entry).await
}

/// Step 3: the object itself under the new key, then the staged copy goes.
async fn finish(
    ports: &dyn RekeyPorts,
    current: &RevfsKey,
    entry: &mut IndexEntry,
    local: &Path,
    staged_path: &str,
) -> Result<(), String> {
    ports
        .push(entry.location, &entry.path, entry.level, local, current)
        .await?;
    ports.delete(entry.location, staged_path).await?;
    entry.key_id = current.id();
    entry.stage = Stage::Settled;
    ports.record(entry).await
}

#[cfg(test)]
#[path = "rekey_fake.rs"]
mod fake;
#[cfg(test)]
#[path = "rekey_tests.rs"]
mod tests;

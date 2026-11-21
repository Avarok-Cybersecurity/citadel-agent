//! The agent's index of the objects an account stored: the SDK lists no account's objects
//! (`citadel_sdk::fs` writes, reads, takes and deletes only), and re-keying them after a phrase
//! regeneration (rekey.rs) has to know every one. Each entry names where the object is (the
//! server, or a peer), its path, its level, and the key it is sealed under.
//!
//! Kept in the account's byte map beside its roots (account.rs), and updated when a store
//! completes and when a delete or a take succeeds. Paths are not secret from this device; key
//! ids reveal nothing about a key. Positional (bincode) behind a format byte: a root of
//! `ci/persisted-formats.txt`.

use super::roots::KeyId;
use async_trait::async_trait;
use citadel_sdk::prelude::{NodeRemote, Ratchet};
use serde::{Deserialize, Serialize};

pub(crate) const INDEX_FORMAT_V1: u8 = 1;
const KEY: &str = "_AGENT_REVFS_SEED";
const SUB_KEY: &str = "index";
const SELF_PEER_CID: u64 = 0;

/// Where an object is stored.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Location {
    Server,
    Peer(u64),
}

/// How far an object's re-key has come.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    /// The object at its path is sealed under the entry's `key_id`.
    Settled,
    /// A copy sealed under `key_id` is complete at the staging path (rekey.rs `staging`); the
    /// object at its own path may be either.
    Staged { key_id: KeyId },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexEntry {
    pub location: Location,
    pub path: String,
    /// `SecurityLevel::value`, which the object is stored at again when it is re-keyed.
    pub level: u8,
    pub key_id: KeyId,
    pub stage: Stage,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RevfsIndex {
    pub entries: Vec<IndexEntry>,
}

impl RevfsIndex {
    pub(crate) fn get(&self, location: Location, path: &str) -> Option<&IndexEntry> {
        self.entries
            .iter()
            .find(|entry| entry.location == location && entry.path == path)
    }

    /// Records `entry` in place of any entry for the same object.
    pub(crate) fn upsert(&mut self, entry: IndexEntry) {
        match self
            .entries
            .iter_mut()
            .find(|held| held.location == entry.location && held.path == entry.path)
        {
            Some(held) => *held = entry,
            None => self.entries.push(entry),
        }
    }

    pub(crate) fn remove(&mut self, location: Location, path: &str) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|entry| !(entry.location == location && entry.path == path));
        self.entries.len() != before
    }

    /// Every key an object (or a staged copy of one) is sealed under.
    pub(crate) fn referenced(&self) -> Vec<KeyId> {
        let mut ids = Vec::new();
        for entry in &self.entries {
            ids.push(entry.key_id);
            if let Stage::Staged { key_id } = entry.stage {
                ids.push(key_id);
            }
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, String> {
        let body = bincode2::serialize(self).map_err(|err| err.to_string())?;
        Ok([&[INDEX_FORMAT_V1][..], &body].concat())
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, String> {
        match bytes {
            [INDEX_FORMAT_V1, body @ ..] => bincode2::deserialize(body)
                .map_err(|err| format!("the RE-VFS index is unreadable: {err}")),
            _ => Err("the RE-VFS index is in a format this agent does not read".to_string()),
        }
    }
}

/// Where an account's index is kept.
#[async_trait]
pub(crate) trait IndexRecords: Send + Sync {
    async fn load_index(&self, cid: u64) -> Result<Option<Vec<u8>>, String>;
    async fn store_index(&self, cid: u64, bytes: Vec<u8>) -> Result<(), String>;
}

#[async_trait]
impl<R: Ratchet> IndexRecords for NodeRemote<R> {
    async fn load_index(&self, cid: u64) -> Result<Option<Vec<u8>>, String> {
        self.account_manager()
            .get_persistence_handler()
            .get_byte_map_value(cid, SELF_PEER_CID, KEY, SUB_KEY)
            .await
            .map_err(|err| err.into_string())
    }

    async fn store_index(&self, cid: u64, bytes: Vec<u8>) -> Result<(), String> {
        self.account_manager()
            .get_persistence_handler()
            .store_byte_map_value(cid, SELF_PEER_CID, KEY, SUB_KEY, bytes)
            .await
            .map(|_previous| ())
            .map_err(|err| err.into_string())
    }
}

pub(crate) async fn load(records: &dyn IndexRecords, cid: u64) -> Result<RevfsIndex, String> {
    match records.load_index(cid).await? {
        Some(bytes) => RevfsIndex::decode(&bytes),
        None => Ok(RevfsIndex::default()),
    }
}

/// Reads, changes and writes the account's index. The caller serialises updates
/// (`usage::Accounts::index_lock`), so two completions never write over each other.
pub(crate) async fn update<T>(
    records: &dyn IndexRecords,
    cid: u64,
    change: impl FnOnce(&mut RevfsIndex) -> T,
) -> Result<T, String> {
    let mut index = load(records, cid).await?;
    let changed = change(&mut index);
    records.store_index(cid, index.encode()?).await?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Records(Mutex<HashMap<u64, Vec<u8>>>);

    #[async_trait]
    impl IndexRecords for Records {
        async fn load_index(&self, cid: u64) -> Result<Option<Vec<u8>>, String> {
            Ok(self.0.lock().get(&cid).cloned())
        }
        async fn store_index(&self, cid: u64, bytes: Vec<u8>) -> Result<(), String> {
            self.0.lock().insert(cid, bytes);
            Ok(())
        }
    }

    fn entry(location: Location, path: &str, key_id: u8) -> IndexEntry {
        IndexEntry {
            location,
            path: path.into(),
            level: 0,
            key_id: [key_id; 8],
            stage: Stage::Settled,
        }
    }

    #[tokio::test]
    async fn a_store_is_recorded_once_per_object_and_a_delete_forgets_it() {
        let records = Records::default();
        update(&records, 7, |i| i.upsert(entry(Location::Server, "/a", 1)))
            .await
            .unwrap();
        update(&records, 7, |i| i.upsert(entry(Location::Peer(9), "/a", 1)))
            .await
            .unwrap();
        update(&records, 7, |i| i.upsert(entry(Location::Server, "/a", 2)))
            .await
            .unwrap();
        let index = load(&records, 7).await.unwrap();
        assert_eq!(index.entries.len(), 2, "one entry per place and path");
        assert_eq!(index.get(Location::Server, "/a").unwrap().key_id, [2; 8]);
        assert!(update(&records, 7, |i| i.remove(Location::Server, "/a"))
            .await
            .unwrap());
        let index = load(&records, 7).await.unwrap();
        assert!(index.get(Location::Server, "/a").is_none());
        assert!(index.get(Location::Peer(9), "/a").is_some());
        assert!(load(&records, 8).await.unwrap().entries.is_empty());
    }

    #[test]
    fn a_staged_copys_key_is_referenced_too() {
        let mut index = RevfsIndex::default();
        index.upsert(entry(Location::Server, "/a", 1));
        index.upsert(IndexEntry {
            stage: Stage::Staged { key_id: [3; 8] },
            ..entry(Location::Server, "/b", 1)
        });
        assert_eq!(index.referenced(), vec![[1; 8], [3; 8]]);
    }

    #[test]
    fn the_index_round_trips_and_another_format_is_refused() {
        let mut index = RevfsIndex::default();
        index.upsert(entry(Location::Peer(4), "/x/y.txt", 5));
        let bytes = index.encode().unwrap();
        assert_eq!(bytes[0], INDEX_FORMAT_V1);
        assert_eq!(RevfsIndex::decode(&bytes).unwrap(), index);
        let mut other = bytes;
        other[0] = 2;
        assert!(RevfsIndex::decode(&other).is_err());
    }
}

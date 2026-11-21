//! The chat files an account received, kept in that account's own LocalDB bucket so a
//! click on one opens it after the agent restarts, however many RE-VFS pulls came since.
//!
//! One record per file, under `received:{sender}:{saved path}`: the identity the file had
//! when its reception completed. The key is the whole grant: the account's own bucket says
//! whose file it is, the sender in the key must match the read's peer, and the path is the
//! one the agent itself reported. `received:` is reserved (requests/local_db), so no window
//! can write, read, list or clear one; only the accept hook writes them.
//!
//! The read re-checks everything against the disk (requests/file/read_pulled.rs): inside
//! the transfer root, and still the file recorded, so a replaced file is refused.
//!
//! Bounded at `MAX_RECEIVED_RECORDS`. Past it the oldest records whose files are no longer
//! on disk are dropped; a record whose file is still there is never dropped, because its
//! bytes are what the person would lose the click to. A record is ~200 bytes, so the
//! bucket is bounded by the agent's own transfer folder rather than by a guess at chat
//! history. Pure over its store and the disk check (SBIO).

use crate::kernel::conversations::kv::{ConversationKv, KvResult};
use crate::kernel::requests::file::file_identity::FileIdentity;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The reserved LocalDB prefix of these records.
pub(crate) const RECEIVED_KEY_PREFIX: &str = "received:";
/// Records kept before the sweep drops those whose files are gone.
pub(crate) const MAX_RECEIVED_RECORDS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ReceivedRecord {
    identity: FileIdentity,
    recorded_at_secs: u64,
}

fn key(sender: u64, saved: &Path) -> String {
    format!("{RECEIVED_KEY_PREFIX}{sender}:{}", saved.display())
}

fn saved_path_of(key: &str) -> Option<PathBuf> {
    let rest = key.strip_prefix(RECEIVED_KEY_PREFIX)?;
    let (_, path) = rest.split_once(':')?;
    Some(PathBuf::from(path))
}

/// Records `saved`, received from `sender`, then sweeps past the bound.
pub(crate) async fn record(
    store: &dyn ConversationKv,
    sender: u64,
    saved: &Path,
    identity: FileIdentity,
    recorded_at_secs: u64,
    on_disk: &(dyn Fn(&Path) -> bool + Sync),
) -> KvResult<()> {
    let value = serde_json::to_vec(&ReceivedRecord {
        identity,
        recorded_at_secs,
    })
    .map_err(|e| format!("could not encode a received-file record: {e}"))?;
    store.set(&key(sender, saved), value).await?;
    sweep(store, on_disk).await
}

/// The identity `saved` had when this account received it from `sender`, if it did.
pub(crate) async fn lookup(
    store: &dyn ConversationKv,
    sender: u64,
    saved: &Path,
) -> KvResult<Option<FileIdentity>> {
    match store.get(&key(sender, saved)).await? {
        None => Ok(None),
        Some(bytes) => serde_json::from_slice::<ReceivedRecord>(&bytes)
            .map(|record| Some(record.identity))
            .map_err(|e| format!("a received-file record is unreadable: {e}")),
    }
}

async fn sweep(
    store: &dyn ConversationKv,
    on_disk: &(dyn Fn(&Path) -> bool + Sync),
) -> KvResult<()> {
    let keys: Vec<String> = store
        .keys()
        .await?
        .into_iter()
        .filter(|k| k.starts_with(RECEIVED_KEY_PREFIX))
        .collect();
    let mut over = keys.len().saturating_sub(MAX_RECEIVED_RECORDS);
    if over == 0 {
        return Ok(());
    }
    let mut gone: Vec<(u64, String)> = Vec::new();
    for k in keys {
        let missing = saved_path_of(&k).is_none_or(|path| !on_disk(&path));
        if missing {
            let at = match store.get(&k).await? {
                Some(bytes) => serde_json::from_slice::<ReceivedRecord>(&bytes)
                    .map_or(0, |r| r.recorded_at_secs),
                None => continue,
            };
            gone.push((at, k));
        }
    }
    gone.sort();
    for (_, k) in gone {
        if over == 0 {
            break;
        }
        store.delete(&k).await?;
        over -= 1;
    }
    Ok(())
}

#[cfg(test)]
#[path = "received_files_tests.rs"]
mod tests;

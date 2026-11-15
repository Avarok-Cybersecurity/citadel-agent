//! A chat file this session accepted, made readable into the page, and kept readable.
//!
//! The SDK saves an accepted FileTransfer to the agent's disk, under its original name, and
//! the page learns only the path (ReceptionBeginning). With nothing recording it,
//! `ReadPulledFileChunk` refused it, so a click on a received file could only show that path
//! in the agent's hidden data directory, never hand the browser the file.
//!
//! On completion the file is recorded twice: in the session's own table
//! (`Connection::received_reads`, which RE-VFS pulls never evict and which does not expire),
//! and in the account's store (kernel/received_files.rs), which outlives the agent process.
//! A read the table does not hold is looked up in the store (`resolve`), so a click after a
//! restart still opens the file.
//!
//! The gate after a restart is "was the sender", not "is still a peer": the record in the
//! account's own bucket, keyed by the sender, is the proof that this account received this
//! file from that peer. A live P2P channel is not (the sender may be offline), and nor is a
//! current registration: a file someone sent you stays yours, as it does in Slack, after they
//! leave. Every disk check (inside the transfer root, still the same file) is made again on
//! each read (requests/file/read_pulled.rs).

use crate::kernel::conversations::kv::ConversationKv;
use crate::kernel::pulled_files::PulledFileHook;
use crate::kernel::pulled_reads::{PulledKey, Retention};
use crate::kernel::received_files;
use crate::kernel::requests::file::file_identity::{identify, FileIdentity};
use crate::kernel::Connection;
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::Ratchet;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The session's cache of received files: a working set for in-order reads, not the record
/// (that is the account's store), so evicting from it loses nothing.
pub(crate) const MAX_CACHED_RECEIVED: usize = 256;

/// Records `saved` on `cid`'s session under `key`, with the file's identity now.
/// A file that is no longer a regular file records nothing.
pub(crate) fn record_saved<R: Ratchet>(
    connections: &RwLock<HashMap<u64, Connection<R>>>,
    cid: u64,
    key: PulledKey,
    saved: &Path,
    retention: Retention,
    now: Instant,
) {
    let Some(identity) = identify(saved) else {
        return;
    };
    if let Some(conn) = connections.write().get_mut(&cid) {
        let table = match retention {
            Retention::PullWindow => &mut conn.pulled_reads,
            Retention::Session => &mut conn.received_reads,
        };
        table.record(key, saved.to_path_buf(), identity, retention, now);
    }
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The tick task's hook for an accepted chat transfer from `peer_cid`.
pub(crate) fn on_received<R: Ratchet>(
    connections: Arc<RwLock<HashMap<u64, Connection<R>>>>,
    store: Arc<dyn ConversationKv>,
    cid: u64,
    peer_cid: u64,
) -> PulledFileHook {
    Box::new(move |info| {
        let saved = info.file_path.clone();
        let key = PulledKey {
            peer_cid: Some(peer_cid),
            virtual_directory: saved.clone(),
        };
        record_saved(
            &connections,
            cid,
            key,
            &saved,
            Retention::Session,
            info.picked_at,
        );
        let Some(identity) = identify(&saved) else {
            return;
        };
        tokio::spawn(async move {
            let on_disk = |p: &Path| p.is_file();
            if let Err(e) = received_files::record(
                store.as_ref(),
                peer_cid,
                &saved,
                identity,
                unix_secs(),
                &on_disk,
            )
            .await
            {
                warn!(target: "citadel", "received file {saved:?} for {cid} is readable this session only: {e}");
            }
        });
    })
}

/// The sessions' received tables, as `resolve` reads and fills them.
pub(crate) trait ReceivedTables: Send + Sync {
    fn holds(&self, cid: u64, key: &PulledKey) -> bool;
    fn load(&self, cid: u64, key: &PulledKey, identity: FileIdentity) -> Result<(), String>;
}

impl<R: Ratchet> ReceivedTables for RwLock<HashMap<u64, Connection<R>>> {
    fn holds(&self, cid: u64, key: &PulledKey) -> bool {
        self.read()
            .get(&cid)
            .is_some_and(|conn| conn.received_reads.holds(key))
    }

    fn load(&self, cid: u64, key: &PulledKey, identity: FileIdentity) -> Result<(), String> {
        let mut lock = self.write();
        let conn = lock
            .get_mut(&cid)
            .ok_or_else(|| "read: Server Connection Not Found".to_string())?;
        if !conn.received_reads.holds(key) {
            let saved = key.virtual_directory.clone();
            let now = Instant::now();
            conn.received_reads
                .record(key.clone(), saved, identity, Retention::Session, now);
        }
        Ok(())
    }
}

/// Makes `key` readable from `cid`'s received table, loading it from the account's store
/// when the session has not seen it (the agent restarted since). Ok(false): never received.
pub(crate) async fn resolve(
    tables: &dyn ReceivedTables,
    store: &dyn ConversationKv,
    cid: u64,
    key: &PulledKey,
) -> Result<bool, String> {
    if tables.holds(cid, key) {
        return Ok(true);
    }
    let Some(sender) = key.peer_cid else {
        return Ok(false);
    };
    match received_files::lookup(store, sender, &key.virtual_directory).await? {
        Some(identity) => tables.load(cid, key, identity).map(|()| true),
        None => Ok(false),
    }
}

#[cfg(test)]
#[path = "received_reads_tests.rs"]
mod tests;

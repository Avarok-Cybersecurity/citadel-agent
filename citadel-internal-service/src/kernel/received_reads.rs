//! A chat file this session accepted, made readable into the page.
//!
//! The SDK saves an accepted FileTransfer to the agent's disk, under its
//! original name, and the page learns only the path (ReceptionBeginning). With
//! nothing recording it, `ReadPulledFileChunk` refused it, so a click on a
//! received file could only show that path in the agent's hidden data
//! directory, never hand the browser the file. On completion the file is now
//! recorded on the session (kernel/pulled_reads.rs), named by the path the agent
//! reported and the peer who sent it, for the life of the session.

use crate::kernel::pulled_files::PulledFileHook;
use crate::kernel::pulled_reads::{PulledKey, Retention};
use crate::kernel::requests::file::file_identity::identify;
use crate::kernel::Connection;
use citadel_sdk::prelude::Ratchet;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

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
        conn.pulled_reads
            .record(key, saved.to_path_buf(), identity, retention, now);
    }
}

/// The tick task's hook for an accepted chat transfer from `peer_cid`.
pub(crate) fn on_received<R: Ratchet>(
    connections: Arc<RwLock<HashMap<u64, Connection<R>>>>,
    cid: u64,
    peer_cid: u64,
) -> PulledFileHook {
    Box::new(move |info| {
        let key = PulledKey {
            peer_cid: Some(peer_cid),
            virtual_directory: info.file_path.clone(),
        };
        record_saved(
            &connections,
            cid,
            key,
            &info.file_path,
            Retention::Session,
            info.picked_at,
        );
    })
}

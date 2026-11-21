//! `ReadPulledFileChunk`: one chunk of a file this session pulled, to the page.
//!
//! Which file, and whether this read is in order, is kernel/pulled_reads.rs. This
//! is the disk, behind three gates: the session must be this connection's (the
//! ownership gate in requests/mod.rs), a peer pull's peer must still be one of
//! the session's peers (a received chat file's sender is matched by its record,
//! kernel/received_reads.rs), and the recorded path, canonicalised at read time, must
//! lie inside the SDK's file-transfer directory. The size limit is checked on the
//! opened file before any bytes are read.

use super::file_identity::{is_reparse_point, open_no_follow, FileIdentity};
use crate::kernel::conversations::kv::HostKv;
use crate::kernel::pulled_reads::{plan_chunk, PulledKey};
use crate::kernel::received_reads::resolve;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, ReadPulledFileChunkFailure,
    ReadPulledFileChunkSuccess,
};
use citadel_sdk::prelude::{BackendType, Ratchet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;
use uuid::Uuid;

/// Where the SDK saves a received file under its filesystem backend's home:
/// `{home}/transfers/{source_cid}/{name}` (citadel_user directory_store.rs,
/// `file_transfer_dir`). The SDK does not expose its directory store, so the
/// layout is named here; the round-trip test in tests/ proves a real pull
/// lands inside it.
const SDK_FILE_TRANSFER_SUBDIR: &str = "transfers";

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::ReadPulledFileChunk {
        request_id,
        cid,
        peer_cid,
        virtual_directory,
        offset,
        max_len,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };
    let key = PulledKey {
        peer_cid,
        virtual_directory,
    };
    let response = match read(this, cid, &key, offset, max_len).await {
        Ok((total_size, data)) => {
            InternalServiceResponse::ReadPulledFileChunkSuccess(ReadPulledFileChunkSuccess {
                cid,
                request_id: Some(request_id),
                total_size,
                offset,
                data,
            })
        }
        Err(message) => {
            InternalServiceResponse::ReadPulledFileChunkFailure(ReadPulledFileChunkFailure {
                cid,
                request_id: Some(request_id),
                message,
            })
        }
    };
    Some(HandledRequestResult { response, uuid })
}

/// Which table a read is held in: a RE-VFS pull's, or a chat file the account received.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Pull,
    Received,
}

async fn read<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    key: &PulledKey,
    offset: u64,
    max_len: u32,
) -> Result<(u64, Vec<u8>), String> {
    let pulled = this
        .server_connection_map
        .read()
        .get(&cid)
        .is_some_and(|conn| conn.pulled_reads.holds(key));
    let store = HostKv::account(this.host_io(), cid);
    let source = if !pulled && resolve(&*this.server_connection_map, &store, cid, key).await? {
        Source::Received
    } else {
        Source::Pull
    };
    let ticket = {
        let mut lock = this.server_connection_map.write();
        let conn = lock
            .get_mut(&cid)
            .ok_or_else(|| "read: Server Connection Not Found".to_string())?;
        // A pull's peer must still be a peer. A received file's sender is checked by its record
        // instead (kernel/received_reads.rs): the file stays this account's when they are offline.
        if let (Source::Pull, Some(peer_cid)) = (source, key.peer_cid) {
            if !conn.peers.contains_key(&peer_cid) {
                return Err(format!("peer {peer_cid} is not a peer of this session"));
            }
        }
        let table = match source {
            Source::Pull => &mut conn.pulled_reads,
            Source::Received => &mut conn.received_reads,
        };
        table.begin_read(key, offset, max_len, Instant::now())?
    };
    let generation = ticket.generation;
    let outcome = match sdk_transfer_root(this.remote().account_manager().get_backend_type()) {
        Ok(root) => tokio::task::spawn_blocking(move || {
            if source == Source::Received && !ticket.saved.exists() {
                return Err(GONE.to_string());
            }
            read_confined(&root, &ticket.saved, &ticket.identity, offset, max_len).map_err(|e| {
                if source == Source::Received && e.contains("replaced") {
                    REPLACED.to_string()
                } else {
                    e
                }
            })
        })
        .await
        .map_err(|e| format!("the read did not finish: {e}"))
        .and_then(|read| read),
        Err(e) => Err(e),
    };
    if let Some(conn) = this.server_connection_map.write().get_mut(&cid) {
        let landed = outcome.as_ref().ok().map(|(_, data)| (offset, data.len()));
        let table = match source {
            Source::Pull => &mut conn.pulled_reads,
            Source::Received => &mut conn.received_reads,
        };
        table.settle(key, generation, landed);
    }
    outcome
}

/// A received file the person deleted or moved from the agent's transfer folder.
pub(crate) const GONE: &str =
    "this file is no longer on this computer: it was deleted or moved after it arrived";
/// A received file whose bytes are not the ones received.
pub(crate) const REPLACED: &str =
    "this file was changed or replaced on this computer after it arrived, so it is not opened";

/// The directory the SDK saves received files under, for `backend`.
pub(crate) fn sdk_transfer_root(backend: &BackendType) -> Result<PathBuf, String> {
    match backend {
        BackendType::Filesystem(home) => Ok(Path::new(home).join(SDK_FILE_TRANSFER_SUBDIR)),
        _ => Err("this agent has no filesystem backend, so it holds no pulled files".into()),
    }
}

/// Up to `max_len` bytes of `saved` at `offset`, and its size, if `saved`
/// resolves to a regular file inside `root` that is still the file `expected`
/// identifies.
pub(crate) fn read_confined(
    root: &Path,
    saved: &Path,
    expected: &FileIdentity,
    offset: u64,
    max_len: u32,
) -> Result<(u64, Vec<u8>), String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("the agent's transfer directory is unavailable: {e}"))?;
    let path = saved
        .canonicalize()
        .map_err(|e| format!("the pulled file is gone: {e}"))?;
    if !path.starts_with(&root) {
        return Err("the pulled file is outside the agent's transfer directory".into());
    }
    let mut file =
        open_no_follow(&path).map_err(|e| format!("could not open the pulled file: {e}"))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("could not read the pulled file: {e}"))?;
    if !metadata.is_file() || is_reparse_point(&metadata) {
        return Err("the pulled file is not a regular file".into());
    }
    let opened =
        FileIdentity::of(&file).map_err(|e| format!("could not identify the pulled file: {e}"))?;
    if opened != *expected {
        return Err(
            "the pulled file was replaced after this session pulled it; pull it again (DownloadFile)"
                .into(),
        );
    }
    let total = metadata.len();
    let len = plan_chunk(total, offset, max_len)?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| format!("could not read the pulled file: {e}"))?;
    let mut data = vec![0u8; len];
    file.read_exact(&mut data)
        .map_err(|e| format!("the pulled file changed while it was read: {e}"))?;
    Ok((total, data))
}

#[cfg(test)]
#[path = "read_pulled_tests.rs"]
mod tests;

//! Browser files being staged on the agent chunk by chunk (`StageUploadChunk`).
//!
//! A browser send used to carry the whole file inline in one `SendFile`, capped at
//! 16 MiB. Staging moves it in acknowledged chunks into a private request directory
//! under the browser-transfer root, and `SendFile { source: StagedUpload }` sends the
//! completed copy. This module is the pure part: which chunk may land where, and the
//! session's table of uploads in progress. The disk I/O is in
//! requests/file/staged_upload.rs.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// The largest browser file the agent stages: THE ceiling on a file sent from the
/// browser. It is also the browser-transfer root's aggregate cap, so one file at the
/// ceiling always fits an otherwise empty root. A native-picker send has no ceiling.
pub const MAX_STAGED_UPLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The largest chunk the agent accepts in one `StageUploadChunk`.
pub const MAX_STAGE_CHUNK_BYTES: usize = 1024 * 1024;

/// Bytes promised to an upload but not yet on disk, counted against the root's cap
/// with what is on disk. Shrinks as chunks land; whatever is left is returned on drop.
pub struct Reservation {
    in_flight: Arc<AtomicU64>,
    remaining: u64,
}

impl Reservation {
    pub fn new(in_flight: Arc<AtomicU64>, bytes: u64) -> Self {
        Self {
            in_flight,
            remaining: bytes,
        }
    }

    pub fn landed(&mut self, bytes: u64) {
        let bytes = bytes.min(self.remaining);
        self.in_flight.fetch_sub(bytes, Ordering::SeqCst);
        self.remaining -= bytes;
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.in_flight.fetch_sub(self.remaining, Ordering::SeqCst);
    }
}

pub struct StagedUpload {
    pub dir: PathBuf,
    pub path: PathBuf,
    pub total: u64,
    pub received: u64,
    /// A chunk is being written; a second one for this upload is refused meanwhile.
    pub writing: bool,
    pub started: Instant,
    pub reservation: Reservation,
}

/// What to do with an arriving chunk.
#[derive(Debug, PartialEq, Eq)]
pub enum ChunkPlan {
    /// The first chunk: create the staging file.
    Start,
    /// A later chunk: append it.
    Append,
    Refuse(String),
}

pub fn plan_chunk(
    existing: Option<&StagedUpload>,
    offset: u64,
    len: usize,
    total: u64,
) -> ChunkPlan {
    if total == 0 {
        return ChunkPlan::Refuse("an empty file cannot be staged".into());
    }
    if total > MAX_STAGED_UPLOAD_BYTES {
        return ChunkPlan::Refuse(format!(
            "the file is {total} bytes; files sent from the browser are limited to {MAX_STAGED_UPLOAD_BYTES}"
        ));
    }
    if len == 0 || len > MAX_STAGE_CHUNK_BYTES {
        return ChunkPlan::Refuse(format!(
            "a chunk must hold 1 to {MAX_STAGE_CHUNK_BYTES} bytes"
        ));
    }
    if offset.saturating_add(len as u64) > total {
        return ChunkPlan::Refuse("the chunk runs past the file's declared size".into());
    }
    match existing {
        None if offset == 0 => ChunkPlan::Start,
        None => ChunkPlan::Refuse("no staged upload with that id on this session".into()),
        Some(upload) if upload.total != total => {
            ChunkPlan::Refuse("the chunk declares a different file size".into())
        }
        Some(upload) if upload.writing => {
            ChunkPlan::Refuse("the previous chunk is still being written".into())
        }
        Some(upload) if offset != upload.received => ChunkPlan::Refuse(format!(
            "chunk at {offset}, but {} bytes have been received",
            upload.received
        )),
        Some(_) => ChunkPlan::Append,
    }
}

/// One session's uploads in progress, by the browser's upload id.
#[derive(Default)]
pub struct StagedUploads {
    by_id: HashMap<Uuid, StagedUpload>,
}

impl StagedUploads {
    pub fn get(&self, id: &Uuid) -> Option<&StagedUpload> {
        self.by_id.get(id)
    }

    pub fn get_mut(&mut self, id: &Uuid) -> Option<&mut StagedUpload> {
        self.by_id.get_mut(id)
    }

    /// Refuses to replace an upload already in progress under `id`.
    pub fn insert(&mut self, id: Uuid, upload: StagedUpload) -> Result<(), StagedUpload> {
        if self.by_id.contains_key(&id) {
            return Err(upload);
        }
        self.by_id.insert(id, upload);
        Ok(())
    }

    pub fn remove(&mut self, id: &Uuid) -> Option<StagedUpload> {
        self.by_id.remove(id)
    }

    /// The completed upload `id`, removed from the table, for sending.
    pub fn take_complete(&mut self, id: &Uuid) -> Result<StagedUpload, String> {
        match self.by_id.get(id) {
            None => Err("no staged upload with that id on this session".into()),
            Some(upload) if upload.writing || upload.received < upload.total => Err(format!(
                "the staged upload holds {} of {} bytes",
                upload.received, upload.total
            )),
            Some(_) => Ok(self.by_id.remove(id).expect("checked above")),
        }
    }

    /// Drops uploads started more than `ttl` ago, returning their directories so the
    /// caller can remove them; their reservations return to the cap as they drop.
    pub fn prune(&mut self, now: Instant, ttl: Duration) -> Vec<PathBuf> {
        let stale: Vec<Uuid> = self
            .by_id
            .iter()
            .filter(|(_, u)| now.duration_since(u.started) >= ttl)
            .map(|(id, _)| *id)
            .collect();
        stale
            .into_iter()
            .filter_map(|id| self.by_id.remove(&id).map(|u| u.dir))
            .collect()
    }
}

#[cfg(test)]
#[path = "staged_uploads_tests.rs"]
mod tests;

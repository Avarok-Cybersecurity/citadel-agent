//! Which pulled files a session may read into the page, and where each read is.
//!
//! `ReadPulledFileChunk` hands a pulled file's bytes to the page. The page never
//! names a local path: a completed `DownloadFile` pull records
//! `(peer_cid, virtual_directory) -> saved path` on its session (the tick task's
//! `on_pulled` hook, responses/object_transfer_handle.rs), and a read names the
//! file the same way it was pulled. Anything not recorded was never pulled here.
//!
//! A record lives for the same window and under the same cap as a picked file
//! (kernel/picked_files.rs), and with its session. Pulling the same file again
//! replaces it. Reads go in order: offset 0 (re)starts a read, any other offset
//! must be where the previous chunk ended.
//!
//! Pure: the disk read, and confining the saved path to the agent's transfer
//! root, are requests/file/read_pulled.rs.

use crate::kernel::picked_files::{MAX_PICKED_FILES, PICKED_FILE_TTL};
use crate::kernel::requests::file::upload::MAX_BROWSER_TRANSFER_TOTAL_BYTES;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

/// The most one `ReadPulledFileChunk` may ask for; a larger `max_len` is refused.
pub const MAX_READ_CHUNK_BYTES: u32 = 1024 * 1024;

/// How a read names a pulled file: as its `DownloadFile` did.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PulledKey {
    pub peer_cid: Option<u64>,
    pub virtual_directory: PathBuf,
}

#[derive(Debug)]
struct PulledRead {
    saved: PathBuf,
    recorded_at: Instant,
    /// Where the previous chunk ended; `None` before the first read.
    next_offset: Option<u64>,
}

#[derive(Default, Debug)]
pub struct PulledReads {
    entries: HashMap<PulledKey, PulledRead>,
}

impl PulledReads {
    /// Records the file a completed pull saved, sweeping expired records and
    /// evicting the oldest past the cap.
    pub fn record(&mut self, key: PulledKey, saved: PathBuf, now: Instant) {
        self.entries
            .retain(|_, read| now.duration_since(read.recorded_at) < PICKED_FILE_TTL);
        self.entries.insert(
            key,
            PulledRead {
                saved,
                recorded_at: now,
                next_offset: None,
            },
        );
        while self.entries.len() > MAX_PICKED_FILES {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, read)| read.recorded_at)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => {
                    self.entries.remove(&key);
                }
                None => break,
            }
        }
    }

    /// The saved path a read at `offset` of at most `max_len` bytes may open.
    pub fn begin_read(
        &self,
        key: &PulledKey,
        offset: u64,
        max_len: u32,
        now: Instant,
    ) -> Result<PathBuf, String> {
        if max_len == 0 || max_len > MAX_READ_CHUNK_BYTES {
            return Err(format!(
                "max_len must be between 1 and {MAX_READ_CHUNK_BYTES} bytes, not {max_len}"
            ));
        }
        let read = self.entries.get(key).ok_or_else(|| {
            format!(
                "{} has not been pulled to this agent by this session; pull it first (DownloadFile)",
                key.virtual_directory.display()
            )
        })?;
        if now.duration_since(read.recorded_at) >= PICKED_FILE_TTL {
            return Err(format!(
                "the pull of {} has expired; pull it again (DownloadFile)",
                key.virtual_directory.display()
            ));
        }
        if offset != 0 && read.next_offset != Some(offset) {
            return Err(match read.next_offset {
                Some(next) => {
                    format!("offset {offset} is out of order: the next chunk is at {next}")
                }
                None => format!("offset {offset} is out of order: a read starts at 0"),
            });
        }
        Ok(read.saved.clone())
    }

    /// Notes that `len` bytes were read at `offset`.
    pub fn finish_read(&mut self, key: &PulledKey, offset: u64, len: usize) {
        if let Some(read) = self.entries.get_mut(key) {
            read.next_offset = offset.checked_add(len as u64);
        }
    }
}

/// How many bytes a read at `offset` of a `total`-byte file returns, or why it is refused.
pub fn plan_chunk(total: u64, offset: u64, max_len: u32) -> Result<usize, String> {
    if total > MAX_BROWSER_TRANSFER_TOTAL_BYTES {
        return Err(format!(
            "the file is {total} bytes, over the {MAX_BROWSER_TRANSFER_TOTAL_BYTES} bytes a page may read"
        ));
    }
    // An empty file is read whole by one empty chunk at 0; otherwise the end of
    // the file is past it.
    if offset > total || (offset == total && total > 0) {
        return Err(format!(
            "offset {offset} is past the end of the {total} byte file"
        ));
    }
    let len = (total - offset).min(u64::from(max_len));
    usize::try_from(len).map_err(|_| format!("a {len} byte chunk does not fit in memory"))
}

#[cfg(test)]
#[path = "pulled_reads_tests.rs"]
mod tests;

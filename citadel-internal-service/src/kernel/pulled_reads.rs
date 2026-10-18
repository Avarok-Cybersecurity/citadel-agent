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
//! must be where the previous chunk ended. One read is in flight per record at
//! a time, and a re-pull starts a new generation, so two reads (or a read and a
//! re-pull) cannot interleave chunks of two files.
//!
//! The record also holds the file's identity at completion
//! (requests/file/file_identity.rs): the folder is shared, the file is not.
//!
//! Pure: the disk read, and confining the saved path to the agent's transfer
//! root, are requests/file/read_pulled.rs.

use crate::kernel::picked_files::{MAX_PICKED_FILES, PICKED_FILE_TTL};
use crate::kernel::requests::file::file_identity::FileIdentity;
use crate::kernel::requests::file::upload::MAX_BROWSER_TRANSFER_TOTAL_BYTES;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The most one `ReadPulledFileChunk` may ask for; a larger `max_len` is refused.
pub const MAX_READ_CHUNK_BYTES: u32 = 1024 * 1024;

/// How long a read may hold its record before it is taken as abandoned. One
/// chunk is at most `MAX_READ_CHUNK_BYTES` from local disk, so a read still
/// running after this is not coming back.
pub const READ_IN_FLIGHT_TIMEOUT: Duration = Duration::from_secs(30);

/// How a read names a pulled file: as its `DownloadFile` did.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PulledKey {
    pub peer_cid: Option<u64>,
    pub virtual_directory: PathBuf,
}

#[derive(Debug)]
struct PulledRead {
    saved: PathBuf,
    identity: FileIdentity,
    recorded_at: Instant,
    generation: u64,
    /// Where the previous chunk ended; `None` before the first read.
    next_offset: Option<u64>,
    /// When the read now holding this record began.
    in_flight_since: Option<Instant>,
}

/// What one read may open, and which record it settles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadTicket {
    pub saved: PathBuf,
    pub identity: FileIdentity,
    pub generation: u64,
}

#[derive(Default, Debug)]
pub struct PulledReads {
    entries: HashMap<PulledKey, PulledRead>,
    next_generation: u64,
}

impl PulledReads {
    /// Records the file a completed pull saved, sweeping expired records and
    /// evicting the oldest past the cap.
    pub fn record(&mut self, key: PulledKey, saved: PathBuf, identity: FileIdentity, now: Instant) {
        self.entries
            .retain(|_, read| now.duration_since(read.recorded_at) < PICKED_FILE_TTL);
        self.next_generation = self.next_generation.wrapping_add(1);
        self.entries.insert(
            key,
            PulledRead {
                saved,
                identity,
                recorded_at: now,
                generation: self.next_generation,
                next_offset: None,
                in_flight_since: None,
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

    /// What a read at `offset` of at most `max_len` bytes may open. The record
    /// is held until `settle` names the ticket's generation.
    pub fn begin_read(
        &mut self,
        key: &PulledKey,
        offset: u64,
        max_len: u32,
        now: Instant,
    ) -> Result<ReadTicket, String> {
        if max_len == 0 || max_len > MAX_READ_CHUNK_BYTES {
            return Err(format!(
                "max_len must be between 1 and {MAX_READ_CHUNK_BYTES} bytes, not {max_len}"
            ));
        }
        let read = self.entries.get_mut(key).ok_or_else(|| {
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
        if let Some(since) = read.in_flight_since {
            if now.duration_since(since) < READ_IN_FLIGHT_TIMEOUT {
                return Err(format!(
                    "a read of {} is already in progress; wait for its answer",
                    key.virtual_directory.display()
                ));
            }
        }
        read.in_flight_since = Some(now);
        Ok(ReadTicket {
            saved: read.saved.clone(),
            identity: read.identity.clone(),
            generation: read.generation,
        })
    }

    /// Releases the record a ticket of `generation` holds, noting `landed`
    /// (offset, length) when the read succeeded. A ticket from before a re-pull
    /// touches nothing.
    pub fn settle(&mut self, key: &PulledKey, generation: u64, landed: Option<(u64, usize)>) {
        let Some(read) = self.entries.get_mut(key) else {
            return;
        };
        if read.generation != generation {
            return;
        }
        read.in_flight_since = None;
        if let Some((offset, len)) = landed {
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

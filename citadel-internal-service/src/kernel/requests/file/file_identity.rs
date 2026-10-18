//! What a pulled file IS, as opposed to where it is.
//!
//! Every pull lands in one shared folder, `transfers/{source_cid}/{name}`, so a
//! path does not name one session's file: two local accounts pulling a
//! same-named file from the same source write the same path, and a read
//! confined only to the folder would hand one account the other's plaintext.
//! The pull's completion therefore records the file's identity, and a read
//! compares it with the file it actually opened (`fstat` on the handle). The
//! same comparison catches a path component swapped between resolving the path
//! and opening it: whatever was opened instead is a different file.
//!
//! Unix: device and inode, plus length and modification time (an overwrite in
//! place keeps the inode). Windows: creation time, length and modification
//! time, with reparse points refused at open (std has no stable file index on
//! Windows).

use std::fs::Metadata;
use std::path::Path;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIdentity {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(windows)]
    created: u64,
}

/// `FILE_ATTRIBUTE_REPARSE_POINT`: a symlink or junction on Windows.
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

impl FileIdentity {
    pub fn of(metadata: &Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            dev: std::os::unix::fs::MetadataExt::dev(metadata),
            #[cfg(unix)]
            ino: std::os::unix::fs::MetadataExt::ino(metadata),
            #[cfg(windows)]
            created: std::os::windows::fs::MetadataExt::creation_time(metadata),
        }
    }
}

/// Whether `metadata` describes a reparse point (always false off Windows,
/// where `O_NOFOLLOW` and `is_file` cover the same ground).
pub fn is_reparse_point(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        std::os::windows::fs::MetadataExt::file_attributes(metadata) & FILE_ATTRIBUTE_REPARSE_POINT
            != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

/// The identity of the regular file at `path`, not following a link there.
pub fn identify(path: &Path) -> Option<FileIdentity> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    (metadata.is_file() && !is_reparse_point(&metadata)).then(|| FileIdentity::of(&metadata))
}

#[cfg(test)]
#[path = "file_identity_tests.rs"]
mod tests;

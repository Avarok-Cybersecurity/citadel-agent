//! What a pulled file IS, as opposed to where it is.
//!
//! Every pull lands in one shared folder, `transfers/{source_cid}/{name}`, so a
//! path does not name one session's file: two local accounts pulling a
//! same-named file from the same source write the same path, and a read
//! confined only to the folder would hand one account the other's plaintext.
//! The pull's completion therefore records the file's identity, and a read
//! compares it with the file it actually opened, from the open handle. The
//! same comparison catches a path component swapped between resolving the path
//! and opening it: whatever was opened instead is a different file.
//!
//! The identity is the file's volume and index -- device and inode on Unix,
//! volume serial and file index on Windows (`GetFileInformationByHandle`; std
//! keeps its own accessors unstable) -- plus length and modification time,
//! because an overwrite in place keeps the index. Timestamps alone are not an
//! identity: two files written back to back on Windows share them.

use std::fs::File;
use std::path::Path;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIdentity {
    volume: u64,
    index: u64,
    len: u64,
    modified: Option<SystemTime>,
}

impl FileIdentity {
    /// The identity of an open file, read from its handle.
    pub fn of(file: &File) -> std::io::Result<Self> {
        let metadata = file.metadata()?;
        let (volume, index) = volume_and_index(file, &metadata)?;
        Ok(Self {
            volume,
            index,
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

#[cfg(unix)]
fn volume_and_index(_file: &File, metadata: &std::fs::Metadata) -> std::io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn volume_and_index(file: &File, _metadata: &std::fs::Metadata) -> std::io::Result<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    // SAFETY: the handle is open for the life of `file`, and `info` is a
    // writable BY_HANDLE_FILE_INFORMATION the call fills on success.
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: zero-initialised, and filled by the successful call above.
    let info = unsafe { info.assume_init() };
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Ok((u64::from(info.dwVolumeSerialNumber), index))
}

#[cfg(not(any(unix, windows)))]
fn volume_and_index(_file: &File, _metadata: &std::fs::Metadata) -> std::io::Result<(u64, u64)> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no file identity on this platform",
    ))
}

/// `FILE_ATTRIBUTE_REPARSE_POINT`: a symlink or junction on Windows.
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// `FILE_FLAG_OPEN_REPARSE_POINT`: open a reparse point itself, not its target.
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

/// Whether `metadata` describes a reparse point (always false off Windows,
/// where `O_NOFOLLOW` and `is_file` cover the same ground).
pub fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
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

/// Opens `path` read-only without following a link at its last component:
/// `O_NOFOLLOW` on Unix, `FILE_FLAG_OPEN_REPARSE_POINT` on Windows (the
/// reparse point itself is opened, for the caller to refuse). Non-blocking, so
/// a FIFO planted in the directory cannot hang the open.
pub fn open_no_follow(path: &Path) -> std::io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(
        &mut options,
        libc::O_NOFOLLOW | libc::O_NONBLOCK,
    );
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::custom_flags(&mut options, FILE_FLAG_OPEN_REPARSE_POINT);
    options.open(path)
}

/// The identity of the regular file at `path`, not following a link there.
pub fn identify(path: &Path) -> Option<FileIdentity> {
    let file = open_no_follow(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || is_reparse_point(&metadata) {
        return None;
    }
    FileIdentity::of(&file).ok()
}

#[cfg(test)]
#[path = "file_identity_tests.rs"]
mod tests;

//! Removing a browser upload's payload (`FileSource::ByteContents`) from the transfer root.
//!
//! The payload lives in its own request directory under the root, and the root's 256 MiB
//! aggregate cap counts what is on disk there -- so removing the directory is also what
//! returns its bytes to the cap. It is removed when its transfer ends: completed, failed,
//! or refused before it began. The 10-minute timer and the startup sweep remain, for a
//! transfer that never ends (an offer nobody answers) and for a crash.
//!
//! Before, only the timer removed it, so browser sends failed once about 256 MiB had been
//! sent in any rolling 10 minutes.

use citadel_sdk::logging::{info, warn};
use std::path::{Path, PathBuf};

/// The request directory holding a materialised payload: the file's parent.
pub(crate) fn request_dir_of(payload: &Path) -> Option<PathBuf> {
    payload.parent().map(Path::to_path_buf)
}

/// Removes `dir` and everything in it; already gone is fine.
pub(crate) async fn remove_request_dir(dir: &Path) {
    match tokio::fs::remove_dir_all(dir).await {
        Ok(()) => info!(target: "citadel", "Removed browser payload {:?}", dir),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warn!(target: "citadel", "Failed to remove browser payload {:?}: {}", dir, e),
    }
}

/// Removes `dir` now, off the caller's path.
pub(crate) fn remove_request_dir_soon(dir: PathBuf) {
    tokio::spawn(async move { remove_request_dir(&dir).await });
}

#[cfg(test)]
mod tests {
    use super::{remove_request_dir, request_dir_of};
    use std::path::Path;

    #[test]
    fn the_request_dir_is_the_payload_s_parent() {
        let dir = request_dir_of(Path::new("/root/req-1/photo.png"));
        assert_eq!(dir.as_deref(), Some(Path::new("/root/req-1")));
    }

    #[tokio::test]
    async fn removal_takes_the_whole_request_dir_and_tolerates_absence() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("req-1");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("photo.png"), b"x").unwrap();
        remove_request_dir(&dir).await;
        assert!(!dir.exists());
        remove_request_dir(&dir).await;
    }
}

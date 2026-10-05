//! Where browser-uploaded payloads (`FileSource::ByteContents`) are written before the SDK
//! sends them, and how much is in flight there. Configuration, not a constant: every service
//! is given its root, so the shipped agent names `$TMPDIR` explicitly and each test names a
//! directory of its own. A root shared between processes shared the 256 MiB aggregate cap too:
//! payloads one test run left behind (removed only by a 10-minute timer that the run did not
//! outlive) refused every later run's uploads.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

/// Subdirectory of the system temp dir the shipped agent uses.
const SYSTEM_TEMP_SUBDIR: &str = "citadel-browser-transfers";

#[derive(Clone, Debug)]
pub struct BrowserTransferRoot {
    dir: PathBuf,
    /// Bytes of uploads being written under `dir` (see `upload.rs`, the aggregate cap). With
    /// the root, so two services never count each other's.
    in_flight: Arc<AtomicU64>,
}

impl BrowserTransferRoot {
    /// Payloads go under `dir`, which is created (private) on first use.
    pub fn at(dir: PathBuf) -> Self {
        Self {
            dir,
            in_flight: Arc::new(AtomicU64::new(0)),
        }
    }

    /// `$TMPDIR/citadel-browser-transfers`: the agent binary's root.
    pub fn in_system_temp_dir() -> Self {
        Self::at(std::env::temp_dir().join(SYSTEM_TEMP_SUBDIR))
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn in_flight(&self) -> &Arc<AtomicU64> {
        &self.in_flight
    }
}

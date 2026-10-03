//! Staging for hosts that cannot run the fixture agent.
//!
//! The fixture "agent" in each tarball is a `#!/bin/sh` script, which Windows refuses to run
//! (os error 193). Running the staged file is part of the self-replacing install, and that
//! install only exists on unix (`updater::swap` is `cfg(unix)`, and a Windows agent is offered
//! a link to the MSI). So off unix, and in the tests that ask for it on unix too, `--version`
//! is read from the script instead of run. Everything else is the real `FsStaging`: the real
//! directory, the real hash, the real `tar` extraction.

use async_trait::async_trait;
use citadel_internal_service::updater::io::Staging;
use citadel_internal_service::updater::platform::Method;
use citadel_internal_service::updater::staging::FsStaging;
use semver::Version;
use std::path::{Path, PathBuf};

pub struct ReadsVersion(pub FsStaging);

#[async_trait]
impl Staging for ReadsVersion {
    fn download_path(&self, version: &Version, asset: &str) -> Result<PathBuf, String> {
        self.0.download_path(version, asset)
    }
    async fn sha256(&self, path: &Path) -> Result<String, String> {
        self.0.sha256(path).await
    }
    async fn stage(&self, method: Method, download: &Path) -> Result<PathBuf, String> {
        self.0.stage(method, download).await
    }
    /// What the fixture script would print: the text of its `echo '…'` line.
    async fn version_of(&self, runnable: &Path) -> Result<String, String> {
        let script = std::fs::read_to_string(runnable).map_err(|e| e.to_string())?;
        script
            .lines()
            .find_map(|l| l.strip_prefix("echo '")?.strip_suffix('\''))
            .map(str::to_string)
            .ok_or_else(|| format!("{} is not a fixture agent", runnable.display()))
    }
    fn discard(&self, version: &Version) {
        self.0.discard(version)
    }
    fn failed(&self, version: &Version) -> bool {
        self.0.failed(version)
    }
    fn mark_failed(&self, version: &Version) {
        self.0.mark_failed(version)
    }
}

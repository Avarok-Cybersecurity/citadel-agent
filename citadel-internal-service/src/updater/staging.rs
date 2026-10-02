//! `Staging` on disk: `<root>/<version>/<asset>` for the download, `<root>/<version>/staged/` for
//! its runnable form, `<root>/failed-<version>` for a version whose install was rolled back.
//!
//! The root is the updater's alone (a cache directory), never the account's data directory.

use super::io::Staging;
use super::platform::Method;
use async_trait::async_trait;
use citadel_sdk::logging::warn;
use semver::Version;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `--version` answers at once; one that does not is not a working agent.
const VERSION_TIMEOUT: Duration = Duration::from_secs(30);
const BINARY: &str = "citadel-agent";

pub struct FsStaging {
    root: PathBuf,
}

impl FsStaging {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn failed_marker(root: &Path, version: &str) -> PathBuf {
        root.join(format!("failed-{version}"))
    }

    fn version_dir(&self, version: &Version) -> PathBuf {
        self.root.join(version.to_string())
    }
}

fn io_err(path: &Path) -> impl Fn(std::io::Error) -> String + '_ {
    move |e| format!("{}: {e}", path.display())
}

#[async_trait]
impl Staging for FsStaging {
    fn download_path(&self, version: &Version, asset: &str) -> Result<PathBuf, String> {
        std::fs::create_dir_all(&self.root).map_err(io_err(&self.root))?;
        let keep = version.to_string();
        for entry in std::fs::read_dir(&self.root).map_err(io_err(&self.root))? {
            let entry = entry.map_err(io_err(&self.root))?;
            if entry.path().is_dir() && entry.file_name() != keep.as_str() {
                if let Err(e) = std::fs::remove_dir_all(entry.path()) {
                    warn!(target: "citadel::update", "an old download was not removed: {e}");
                }
            }
        }
        let dir = self.version_dir(version);
        std::fs::create_dir_all(&dir).map_err(io_err(&dir))?;
        Ok(dir.join(asset))
    }

    async fn sha256(&self, path: &Path) -> Result<String, String> {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let mut file = std::fs::File::open(&path).map_err(io_err(&path))?;
            let mut hasher = Sha256::new();
            let mut buf = vec![0u8; 1 << 16];
            loop {
                let n = file.read(&mut buf).map_err(io_err(&path))?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
            Ok(hex::encode(hasher.finalize()))
        })
        .await
        .map_err(|e| format!("hashing stopped: {e}"))?
    }

    async fn stage(&self, method: Method, download: &Path) -> Result<PathBuf, String> {
        match method {
            Method::MacApp => Ok(download.to_path_buf()),
            Method::AppImage => {
                set_executable(download)?;
                Ok(download.to_path_buf())
            }
            Method::Tarball => {
                let dir = download.with_file_name("staged");
                if dir.exists() {
                    std::fs::remove_dir_all(&dir).map_err(io_err(&dir))?;
                }
                std::fs::create_dir_all(&dir).map_err(io_err(&dir))?;
                // Only the binary, by name: nothing else in the archive is written anywhere.
                let status = tokio::process::Command::new("tar")
                    .arg("-xzf")
                    .arg(download)
                    .arg("-C")
                    .arg(&dir)
                    .arg(format!("./{BINARY}"))
                    .status()
                    .await
                    .map_err(|e| format!("tar did not run: {e}"))?;
                let binary = dir.join(BINARY);
                let regular = std::fs::symlink_metadata(&binary).is_ok_and(|m| m.is_file());
                if !status.success() || !regular {
                    return Err(format!("the archive holds no {BINARY} ({status})"));
                }
                set_executable(&binary)?;
                Ok(binary)
            }
        }
    }

    async fn version_of(&self, runnable: &Path) -> Result<String, String> {
        let run = tokio::process::Command::new(runnable)
            .arg("--version")
            .kill_on_drop(true)
            .output();
        let output = tokio::time::timeout(VERSION_TIMEOUT, run)
            .await
            .map_err(|_| format!("{} --version did not answer", runnable.display()))?
            .map_err(|e| format!("{} did not run: {e}", runnable.display()))?;
        if !output.status.success() {
            return Err(format!(
                "{} --version exited {}",
                runnable.display(),
                output.status
            ));
        }
        String::from_utf8(output.stdout).map_err(|_| "--version printed non-UTF-8".to_string())
    }

    fn discard(&self, version: &Version) {
        let dir = self.version_dir(version);
        if dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                warn!(target: "citadel::update", "{} was not removed: {e}", dir.display());
            }
        }
    }

    fn failed(&self, version: &Version) -> bool {
        Self::failed_marker(&self.root, &version.to_string()).exists()
    }

    fn mark_failed(&self, version: &Version) {
        let marker = Self::failed_marker(&self.root, &version.to_string());
        let written =
            std::fs::create_dir_all(&self.root).and_then(|()| std::fs::write(&marker, b""));
        if let Err(e) = written {
            warn!(target: "citadel::update", "{} was not written: {e}", marker.display());
        }
    }
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(io_err(path))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
#[path = "staging_tests.rs"]
mod tests;

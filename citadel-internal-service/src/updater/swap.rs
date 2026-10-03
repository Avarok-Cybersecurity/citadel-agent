//! The self-replacing installer (unix: a tarball's binary, an AppImage).
//!
//! The new file is copied beside the old one, so it is on the same filesystem; the old one is
//! kept as a hard link; one rename puts the new one in place, atomically. The old binary is then
//! started as the watchdog (watchdog.rs) and this process exits, freeing the port.

use super::io::Installer;
use super::staging::FsStaging;
use super::watch_os::detached;
use super::watchdog::{WatchPlan, WATCHDOG_ENV};
use semver::Version;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

pub struct SelfReplace {
    /// The executable, or the AppImage.
    pub target: PathBuf,
    /// What the new version is started with.
    pub args: Vec<String>,
    pub health: SocketAddr,
    pub staging_root: PathBuf,
    /// Called once the watchdog has the swap: the real installer exits the process.
    pub handed_over: fn(),
}

/// The paths a swap of `target` uses: the incoming copy and the backup, both beside it.
pub fn beside(target: &Path) -> Result<(PathBuf, PathBuf), String> {
    let dir = target
        .parent()
        .ok_or_else(|| format!("{} has no directory", target.display()))?;
    let name = target
        .file_name()
        .ok_or_else(|| format!("{} has no file name", target.display()))?
        .to_string_lossy();
    Ok((
        dir.join(format!(".{name}.incoming")),
        dir.join(format!(".{name}.previous")),
    ))
}

/// Put `staged` at `target`, keeping the old file as the returned backup. On any error the
/// target is as it was.
pub fn swap_in(staged: &Path, target: &Path) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    let (incoming, backup) = beside(target)?;
    let fail = |what: &str, e: std::io::Error| {
        let _ = std::fs::remove_file(&incoming);
        format!("{what}: {e}")
    };
    std::fs::copy(staged, &incoming).map_err(|e| fail("copying the new version in", e))?;
    std::fs::set_permissions(&incoming, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| fail("making it executable", e))?;
    std::fs::File::open(&incoming)
        .and_then(|f| f.sync_all())
        .map_err(|e| fail("flushing it", e))?;
    match std::fs::remove_file(&backup) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(fail("removing the last backup", e)),
    }
    std::fs::hard_link(target, &backup).map_err(|e| fail("keeping the current version", e))?;
    std::fs::rename(&incoming, target).map_err(|e| fail("putting the new version in place", e))?;
    Ok(backup)
}

impl Installer for SelfReplace {
    fn can_install(&self) -> Result<(), String> {
        Ok(())
    }

    fn install(&self, staged: &Path, version: &Version) -> Result<(), String> {
        let backup = swap_in(staged, &self.target)?;
        let plan = WatchPlan {
            parent_pid: std::process::id(),
            target: self.target.clone(),
            backup: backup.clone(),
            health: self.health,
            version: version.to_string(),
            args: self.args.clone(),
            failed_marker: FsStaging::failed_marker(&self.staging_root, &version.to_string()),
        };
        let encoded = serde_json::to_string(&plan).map_err(|e| e.to_string())?;
        let started = detached(&backup, &[]).env(WATCHDOG_ENV, encoded).spawn();
        if let Err(e) = started {
            // Nothing would check the new version, so it does not stay.
            std::fs::rename(&backup, &self.target).map_err(|r| {
                format!("the watchdog did not start ({e}), and restoring failed: {r}")
            })?;
            return Err(format!("the watchdog did not start: {e}"));
        }
        citadel_sdk::logging::error!(target: "citadel::update",
            "restarting into {version}; signed-in accounts will need to sign in again");
        eprintln!("citadel-agent: restarting into {version}");
        (self.handed_over)();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_file_takes_the_place_and_the_old_one_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("citadel-agent");
        let staged = dir.path().join("staged");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&staged, b"new").unwrap();
        let backup = swap_in(&staged, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(std::fs::read(&backup).unwrap(), b"old");
        assert!(
            !beside(&target).unwrap().0.exists(),
            "no incoming copy is left"
        );
        std::fs::write(&staged, b"newer").unwrap();
        let backup = swap_in(&staged, &target).unwrap();
        assert_eq!(
            std::fs::read(&backup).unwrap(),
            b"new",
            "the backup is the last version"
        );
    }

    #[test]
    fn a_failed_swap_leaves_the_target_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("citadel-agent");
        std::fs::write(&target, b"old").unwrap();
        assert!(swap_in(&dir.path().join("missing"), &target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert!(!beside(&target).unwrap().1.exists());
    }
}

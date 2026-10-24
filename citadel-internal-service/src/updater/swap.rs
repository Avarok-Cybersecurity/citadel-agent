//! The self-replacing installer (unix: a tarball's binary, an AppImage).
//!
//! The new file is copied beside the old one, so it is on the same filesystem; the old one is
//! kept as a hard link; one rename puts the new one in place, atomically. The old binary is then
//! started as the watchdog (watchdog.rs) and this process exits, freeing the port.

use super::io::Installer;
use super::launch_check::check_launch;
use super::staging::FsStaging;
use super::watch_os::detached;
use super::watchdog::{WatchPlan, WATCHDOG_ENV};
use parking_lot::Mutex;
use semver::Version;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

pub struct SelfReplace {
    /// The executable, or the AppImage.
    target: PathBuf,
    /// What the new version is started with.
    args: Vec<String>,
    health: SocketAddr,
    staging_root: PathBuf,
    /// Called once the watchdog has the swap: the real installer exits the process.
    handed_over: fn(),
    /// The launch check's answer for the last staged file asked about. Neither the file nor
    /// this agent's command changes while it runs, and the status asks on every read.
    checked: Mutex<Option<(PathBuf, Result<(), String>)>>,
}

impl SelfReplace {
    pub fn new(
        target: PathBuf,
        args: Vec<String>,
        health: SocketAddr,
        staging_root: PathBuf,
        handed_over: fn(),
    ) -> Self {
        Self {
            target,
            args,
            health,
            staging_root,
            handed_over,
            checked: Mutex::new(None),
        }
    }
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
    fn can_install(&self, staged: &Path) -> Result<(), String> {
        let mut checked = self.checked.lock();
        if let Some((_, answer)) = checked.as_ref().filter(|(path, _)| path == staged) {
            return answer.clone();
        }
        let answer = check_launch(staged, &self.args);
        *checked = Some((staged.to_path_buf(), answer.clone()));
        answer
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

    /// A staged "agent" that records each run's arguments, says `said` on stderr, and exits
    /// with `code`.
    fn staged_agent(dir: &Path, said: &str, code: u8) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let staged = dir.join("citadel-agent");
        let runs = dir.join("runs");
        let body = format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\necho '{said}' >&2\nexit {code}\n",
            runs.display()
        );
        std::fs::write(&staged, body).unwrap();
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).unwrap();
        staged
    }

    fn installer(dir: &Path) -> SelfReplace {
        SelfReplace::new(
            dir.join("installed"),
            vec!["--bind".to_string(), "127.0.0.1:12345".to_string()],
            "127.0.0.1:12345".parse().unwrap(),
            dir.to_path_buf(),
            || unreachable!("nothing is installed"),
        )
    }

    fn runs(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("runs"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn a_staged_agent_that_would_refuse_this_agents_command_is_not_installable() {
        let dir = tempfile::tempdir().unwrap();
        let staged = staged_agent(dir.path(), "apps.private_network is not configured", 1);
        let why = installer(dir.path()).can_install(&staged).unwrap_err();
        assert!(
            why.contains("apps.private_network is not configured"),
            "{why}"
        );
        assert_eq!(runs(dir.path()), ["--check-launch --bind 127.0.0.1:12345"]);
    }

    #[test]
    fn a_staged_agent_that_would_start_is_installable() {
        let dir = tempfile::tempdir().unwrap();
        let staged = staged_agent(dir.path(), "", 0);
        assert_eq!(installer(dir.path()).can_install(&staged), Ok(()));
    }

    #[test]
    fn each_staged_file_is_checked_once() {
        // The status asks on every read; each ask must not start the staged agent again.
        let dir = tempfile::tempdir().unwrap();
        let staged = staged_agent(dir.path(), "refused", 1);
        let installer = installer(dir.path());
        for _ in 0..3 {
            assert!(installer.can_install(&staged).is_err());
        }
        assert_eq!(runs(dir.path()).len(), 1);
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

//! The recorders for what would restart the agent, and the world a test runs in.

use super::staging::ReadsVersion;
use super::{FakeSource, FakeVerifier};
use async_trait::async_trait;
use citadel_internal_service::updater::engine::Engine;
use citadel_internal_service::updater::io::*;
use citadel_internal_service::updater::platform::Plan;
use citadel_internal_service::updater::staging::FsStaging;
use citadel_internal_service_types::UpdateAvailable;
use parking_lot::Mutex;
use semver::Version;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Default)]
pub struct Recorder {
    pub installs: Mutex<Vec<(PathBuf, String)>>,
    pub announced: Mutex<Vec<UpdateAvailable>>,
    pub auto: Mutex<Option<bool>>,
    pub signed_in: AtomicUsize,
}

impl Installer for Recorder {
    fn can_install(&self) -> Result<(), String> {
        Ok(())
    }
    fn install(&self, staged: &Path, version: &Version) -> Result<(), String> {
        self.installs
            .lock()
            .push((staged.to_path_buf(), version.to_string()));
        Ok(())
    }
}
impl Announcer for Recorder {
    fn announce(&self, update: &UpdateAvailable) {
        self.announced.lock().push(update.clone());
    }
}
#[async_trait]
impl SettingsStore for Recorder {
    async fn auto_install(&self) -> Result<Option<bool>, String> {
        Ok(*self.auto.lock())
    }
    async fn set_auto_install(&self, on: bool) -> Result<(), String> {
        *self.auto.lock() = Some(on);
        Ok(())
    }
}
impl Sessions for Recorder {
    fn signed_in(&self) -> usize {
        self.signed_in.load(Ordering::SeqCst)
    }
}

pub struct SessionCount(Arc<Recorder>);
impl SessionCount {
    pub fn set(&self, n: usize) {
        self.0.signed_in.store(n, Ordering::SeqCst);
    }
}

pub struct World {
    pub engine: Engine,
    pub source: Arc<FakeSource>,
    pub sessions: SessionCount,
    pub agent_script: String,
    recorder: Arc<Recorder>,
    plan: Plan,
    runs_agent: bool,
    dir: tempfile::TempDir,
}

/// A release-shaped tarball holding `./citadel-agent`, a script printing `prints`.
fn tarball(dir: &Path, script: &str) -> Vec<u8> {
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("citadel-agent"), script).unwrap();
    std::fs::write(src.join("README.md"), "readme").unwrap();
    let out = dir.join("fixture.tar.gz");
    let ok = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&out)
        .arg("-C")
        .arg(&src)
        .args(["./citadel-agent", "./README.md"])
        .status()
        .unwrap()
        .success();
    assert!(ok);
    std::fs::read(out).unwrap()
}

impl World {
    /// The staged agent is run where it can be (unix) and read elsewhere (staging.rs).
    pub fn new(tag: &str, plan: Plan, prints: &str) -> Self {
        Self::with(tag, plan, prints, cfg!(unix))
    }

    /// The staged agent's version is read, not run, on every host.
    pub fn reading_versions(tag: &str, plan: Plan, prints: &str) -> Self {
        Self::with(tag, plan, prints, false)
    }

    fn with(tag: &str, plan: Plan, prints: &str, runs_agent: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let agent_script = format!("#!/bin/sh\necho '{prints}'\n");
        let payload = tarball(dir.path(), &agent_script);
        let names = [
            "citadel-agent-linux-x64.tar.gz",
            "citadel-agent-linux-x64.deb",
            "Citadel-Agent.dmg",
            "Citadel-Agent-x64.msi",
        ];
        let source = Arc::new(FakeSource::new(tag, &names, &payload));
        let recorder = Arc::new(Recorder::default());
        let mut world = Self {
            engine: Engine::new(
                Version::new(0, 0, 0),
                plan.clone(),
                io(&source, &recorder, dir.path(), runs_agent),
            ),
            sessions: SessionCount(recorder.clone()),
            source,
            agent_script,
            recorder,
            plan,
            runs_agent,
            dir,
        };
        world.engine = world.rebuild();
        world
    }

    /// A fresh engine over the same server, recorders and directory.
    pub fn rebuild(&self) -> Engine {
        let io = io(
            &self.source,
            &self.recorder,
            self.dir.path(),
            self.runs_agent,
        );
        Engine::new(Version::new(0, 8, 8), self.plan.clone(), io)
    }

    /// An engine over this world, but with `plan` and `installer`: the production ones a
    /// host's facts select (updater::platform, updater::installer_for).
    pub fn engine_for(&self, plan: Plan, installer: Arc<dyn Installer>) -> Engine {
        let mut io = io(
            &self.source,
            &self.recorder,
            self.dir.path(),
            self.runs_agent,
        );
        io.installer = installer;
        Engine::new(Version::new(0, 8, 8), plan, io)
    }

    pub fn announced(&self) -> Vec<UpdateAvailable> {
        self.recorder.announced.lock().clone()
    }
    /// The recorder, as an installer (the menu-bar app's seam, in tests/updater_platforms.rs).
    pub fn recorder(&self) -> Arc<Recorder> {
        self.recorder.clone()
    }
    pub fn installed(&self) -> Vec<(PathBuf, String)> {
        self.recorder.installs.lock().clone()
    }
    pub fn staged_dir_exists(&self, version: &str) -> bool {
        self.dir.path().join("cache").join(version).exists()
    }
}

fn io(source: &Arc<FakeSource>, recorder: &Arc<Recorder>, dir: &Path, runs_agent: bool) -> Io {
    let staging = FsStaging::new(dir.join("cache"));
    let staging: Arc<dyn Staging> = if runs_agent {
        Arc::new(staging)
    } else {
        Arc::new(ReadsVersion(staging))
    };
    Io {
        source: source.clone(),
        verifier: Arc::new(FakeVerifier),
        staging,
        installer: recorder.clone(),
        announcer: recorder.clone(),
        settings: recorder.clone(),
        sessions: recorder.clone(),
        now: || 1_790_000_000,
    }
}

//! The agent's updater: finds a newer release on GitHub, verifies it, tells the user, and
//! installs it when they ask or when nobody is signed in to be signed out.
//!
//! Pure, and tested on their own: version.rs (tags, semver), release.rs (the release, its URLs),
//! platform.rs (install type, asset), verify.rs (the verdict), policy.rs (when to install),
//! attest.rs's identity check, watchdog.rs's supervision. Behind the seams in io.rs: github.rs,
//! attest.rs, staging.rs, host.rs, swap.rs, watch_os.rs, and the agent's side in kernel/updates.rs.
//! docs/plans/agent-auto-update.md in citadel-workspace has the design.

pub mod attest;
pub mod engine;
mod engine_status;
mod fetch;
pub mod github;
pub mod host;
pub mod io;
pub mod platform;
pub mod policy;
pub mod release;
pub mod staging;
#[cfg(unix)]
pub mod swap;
pub mod verify;
pub mod version;
#[cfg(unix)]
pub mod watch_os;
pub mod watchdog;

use engine::{Engine, LOG_TARGET};
use io::Installer;
use platform::{InstallKind, Plan};
use policy::Trigger;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// How often the latest release is read, after the read at start.
pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// How often a ready update is reconsidered, so one waits only as long as somebody is signed in.
pub const RECONSIDER_EVERY: Duration = Duration::from_secs(10 * 60);

/// What the binary that ships knows and this crate does not. Every field is required.
#[derive(Debug, Clone)]
pub struct UpdaterConfig {
    /// `CARGO_PKG_VERSION` of the shipped crate: what `--version` prints and the tag repeats.
    pub current_version: String,
    /// The updater's own directory (downloads, staged copies, failure markers).
    pub cache_dir: PathBuf,
    /// The agent's socket, which the watchdog probes.
    pub bind: SocketAddr,
    /// The arguments this agent was started with, to start the new version with.
    pub relaunch_args: Vec<String>,
    /// The menu-bar app started this agent (its launch token is in the environment).
    pub launched_by_app: bool,
}

/// Refuses: this install is offered a download link and nothing else.
pub struct LinkOnly(pub String);

impl Installer for LinkOnly {
    fn can_install(&self) -> Result<(), String> {
        Err(self.0.clone())
    }
    fn install(&self, _staged: &Path, _version: &semver::Version) -> Result<(), String> {
        Err(self.0.clone())
    }
}

/// How this agent would be replaced: the install kind, the asset, and the installer for it.
/// `mac_app` is the menu-bar app's installer, which the agent's side builds.
pub fn installer_for(
    config: &UpdaterConfig,
    kind: &InstallKind,
    plan: &Plan,
    mac_app: Arc<dyn Installer>,
) -> Arc<dyn Installer> {
    if let Plan::LinkOnly { why, .. } = plan {
        return Arc::new(LinkOnly(why.clone()));
    }
    #[cfg(unix)]
    let replace = |target: &Path, args: Vec<String>| -> Arc<dyn Installer> {
        Arc::new(swap::SelfReplace {
            target: target.to_path_buf(),
            args,
            health: config.bind,
            staging_root: config.cache_dir.clone(),
            handed_over: || std::process::exit(0),
        })
    };
    match kind {
        InstallKind::MacApp { .. } => mac_app,
        #[cfg(unix)]
        InstallKind::Binary { exe } => replace(exe, config.relaunch_args.clone()),
        // The AppImage's wrapper supplies the flags; passing ours too would repeat them.
        #[cfg(unix)]
        InstallKind::AppImage { image } => replace(image, Vec::new()),
        other => Arc::new(LinkOnly(format!("{other:?} is not installed by the agent"))),
    }
}

/// The updater's life: a check at start and every `CHECK_EVERY`, a reconsideration every
/// `RECONSIDER_EVERY`. Never returns.
pub async fn run(engine: Arc<Engine>) {
    engine.load_settings().await;
    let mut check = tokio::time::interval(CHECK_EVERY);
    let mut reconsider = tokio::time::interval(RECONSIDER_EVERY);
    check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    reconsider.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = check.tick() => engine.check().await,
            _ = reconsider.tick() => {}
        }
        if let Err(e) = engine.consider(Trigger::Idle).await {
            citadel_sdk::logging::error!(target: LOG_TARGET, "{e}");
        }
    }
}

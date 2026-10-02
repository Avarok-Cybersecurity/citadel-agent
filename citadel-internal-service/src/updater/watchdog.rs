//! The watchdog: the OLD agent binary, started by the old agent as it exits, which starts the new
//! one and keeps it only if its loopback socket answers. Otherwise it puts the old binary back
//! (a rename) and starts that again. Restoring is done by the version known to work.
//!
//! Selected by an environment variable rather than a flag, so no argument the agent takes is
//! needed and a wrapper that adds its own flags (the AppImage's) passes it through unchanged.

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const WATCHDOG_ENV: &str = "CITADEL_UPDATE_WATCHDOG";
/// How long the old agent has to exit and free its port.
pub const PARENT_EXIT: Duration = Duration::from_secs(30);
/// How long the new agent has to answer on its socket. A cold start opens its stores and binds
/// in well under a second; this is generous for a slow disk, not a tuned figure.
pub const HEALTHY_WITHIN: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchPlan {
    pub parent_pid: u32,
    /// Where the new version now is; the backup goes back here on failure.
    pub target: PathBuf,
    pub backup: PathBuf,
    pub health: SocketAddr,
    pub version: String,
    /// What `target` is started with (none for an AppImage, whose wrapper adds the flags).
    pub args: Vec<String>,
    pub failed_marker: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Healthy,
    RolledBack,
    /// The old agent never exited, so the new one was not started; the old binary is back.
    ParentStuck,
}

pub trait Child {
    fn exited(&mut self) -> bool;
    fn kill(&mut self);
}

pub trait WatchIo {
    fn alive(&self, pid: u32) -> bool;
    fn spawn(&self, program: &Path, args: &[String]) -> Result<Box<dyn Child>, String>;
    fn healthy(&self, addr: SocketAddr) -> bool;
    fn rename(&self, from: &Path, to: &Path) -> Result<(), String>;
    fn mark(&self, path: &Path);
    fn now(&self) -> Instant;
    /// Between two probes.
    fn pause(&self);
    fn log(&self, line: &str);
}

pub fn supervise(plan: &WatchPlan, io: &dyn WatchIo) -> Outcome {
    let deadline = io.now() + PARENT_EXIT;
    while io.alive(plan.parent_pid) {
        if io.now() >= deadline {
            io.log("the old agent did not exit; restoring it and starting nothing");
            restore(plan, io);
            return Outcome::ParentStuck;
        }
        io.pause();
    }
    let why = match io.spawn(&plan.target, &plan.args) {
        Err(e) => format!("the new agent did not start: {e}"),
        Ok(mut child) => match wait_healthy(plan, io, child.as_mut()) {
            Ok(()) => {
                io.log(&format!("{} answers on {}", plan.version, plan.health));
                return Outcome::Healthy;
            }
            Err(why) => {
                child.kill();
                why
            }
        },
    };
    io.log(&format!("rolling back {}: {why}", plan.version));
    restore(plan, io);
    if let Err(e) = io.spawn(&plan.target, &plan.args) {
        io.log(&format!("the restored agent did not start: {e}"));
    }
    Outcome::RolledBack
}

fn wait_healthy(plan: &WatchPlan, io: &dyn WatchIo, child: &mut dyn Child) -> Result<(), String> {
    let deadline = io.now() + HEALTHY_WITHIN;
    loop {
        if child.exited() {
            return Err("it exited".to_string());
        }
        if io.healthy(plan.health) {
            return Ok(());
        }
        if io.now() >= deadline {
            return Err(format!(
                "{} did not answer within {HEALTHY_WITHIN:?}",
                plan.health
            ));
        }
        io.pause();
    }
}

fn restore(plan: &WatchPlan, io: &dyn WatchIo) {
    io.mark(&plan.failed_marker);
    if let Err(e) = io.rename(&plan.backup, &plan.target) {
        io.log(&format!("the previous agent could not be put back: {e}"));
    }
}

/// When this process was started as a watchdog: supervise, and return the exit code.
pub fn run_if_requested() -> Option<i32> {
    let encoded = std::env::var(WATCHDOG_ENV).ok()?;
    let plan: WatchPlan = match serde_json::from_str(&encoded) {
        Ok(plan) => plan,
        Err(e) => {
            eprintln!("[citadel-agent watchdog] unreadable plan: {e}");
            return Some(2);
        }
    };
    #[cfg(unix)]
    {
        Some(match supervise(&plan, &super::watch_os::OsWatch) {
            Outcome::Healthy => 0,
            Outcome::RolledBack => 1,
            Outcome::ParentStuck => 2,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = plan;
        Some(2)
    }
}

#[cfg(test)]
#[path = "watchdog_tests.rs"]
mod tests;

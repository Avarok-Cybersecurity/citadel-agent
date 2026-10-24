//! What the member is told when an update cannot stay: before installing, when the staged
//! version would refuse this agent's command (updater/launch_check.rs, through the
//! installer's `can_install`), and after a rollback, when the agent put back reads the reason
//! the watchdog recorded.
//!
//! Mocks, and why: as tests/updater_engine.rs. The installer here refuses with a fixed reason;
//! the real one's launch check runs a staged agent and is tested in updater/swap.rs.

#[allow(dead_code)]
#[path = "updater_fakes/mod.rs"]
mod fakes;

use citadel_internal_service::updater::io::Installer;
use citadel_internal_service::updater::platform::{Method, Plan};
use citadel_internal_service::updater::policy::Trigger;
use fakes::*;
use semver::Version;
use std::path::Path;
use std::sync::Arc;

const REFUSAL: &str = "no STUN servers configured";

fn tarball_plan() -> Plan {
    Plan::Apply {
        asset: "citadel-agent-linux-x64.tar.gz",
        method: Method::Tarball,
    }
}

/// A self-replacing installer whose launch check said the staged version would not start.
struct WouldNotStart;

impl Installer for WouldNotStart {
    fn can_install(&self, _staged: &Path) -> Result<(), String> {
        Err(REFUSAL.to_string())
    }
    fn install(&self, _staged: &Path, version: &Version) -> Result<(), String> {
        panic!("{version} was installed although it would not start");
    }
}

#[tokio::test]
async fn an_update_that_would_not_start_is_not_ready_and_never_installed() {
    let world = World::new("agent-v0.9.0", tarball_plan(), "citadel-agent 0.9.0");
    let engine = world.engine_for(tarball_plan(), Arc::new(WouldNotStart));
    engine.check().await;
    let update = engine.status(None).available.expect("0.9.0 is offered");
    assert!(!update.ready, "{update:?}");
    assert!(update.mldsa_verified, "it was verified and staged");

    engine.consider(Trigger::Idle).await.unwrap();
    let why = engine.consider(Trigger::UserAsked).await.unwrap_err();
    assert_eq!(why, format!("Nothing was installed: {REFUSAL}"));
}

#[tokio::test]
async fn the_agent_put_back_after_a_rollback_says_why_and_does_not_retry() {
    let world = World::new("agent-v0.9.0", tarball_plan(), "citadel-agent 0.9.0");
    std::fs::create_dir_all(world.failed_marker("0.9.0").parent().unwrap()).unwrap();
    std::fs::write(world.failed_marker("0.9.0"), "it exited").unwrap();
    world.engine.check().await;
    let why = world.engine.status(None).last_error.unwrap_or_default();
    assert_eq!(
        why,
        "Installing 0.9.0 failed before, so it is not installed automatically: it exited"
    );
    world.engine.consider(Trigger::Idle).await.unwrap();
    assert!(world.installed().is_empty());
}

#[tokio::test]
async fn without_a_recorded_failure_the_same_update_installs() {
    // The control for both tests above: the same world, nothing refused, nothing recorded.
    let world = World::new("agent-v0.9.0", tarball_plan(), "citadel-agent 0.9.0");
    world.engine.check().await;
    assert_eq!(world.engine.status(None).last_error, None);
    world.engine.consider(Trigger::Idle).await.unwrap();
    assert_eq!(world.installed().len(), 1);
}

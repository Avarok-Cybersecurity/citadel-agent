//! The engine with the plan and installer a host's facts select, as the agent builds them
//! (`updater::platform`, `updater::installer_for`), rather than the explicit ones
//! tests/updater_engine.rs drives. One test per kind of host, on every OS: the facts are data.

// Shared with tests/updater_engine.rs, which uses the rest of it.
#[allow(dead_code)]
#[path = "updater_fakes/mod.rs"]
mod fakes;

use citadel_internal_service::updater::io::Installer;
use citadel_internal_service::updater::platform::{install_kind, plan, Arch, HostFacts, Os, Plan};
use citadel_internal_service::updater::policy::Trigger;
use citadel_internal_service::updater::{installer_for, UpdaterConfig};
use fakes::*;
use std::path::PathBuf;
use std::sync::Arc;

fn host(os: Os, exe: &str) -> HostFacts {
    HostFacts {
        os,
        arch: Arch::X64,
        exe: PathBuf::from(exe),
        appimage: None,
        dpkg_owned: false,
        launched_by_app: false,
        replaceable: true,
    }
}

fn selected(world: &World, facts: &HostFacts) -> (Plan, Arc<dyn Installer>) {
    let config = UpdaterConfig {
        current_version: "0.8.8".to_string(),
        cache_dir: PathBuf::from("/unused"),
        bind: "127.0.0.1:12345".parse().unwrap(),
        relaunch_args: Vec::new(),
        launched_by_app: false,
    };
    let kind = install_kind(facts);
    let plan = plan(&kind, facts.os, facts.arch);
    let mac_app: Arc<dyn Installer> = world.recorder();
    let installer = installer_for(&config, &kind, &plan, mac_app);
    (plan, installer)
}

#[tokio::test]
async fn windows_is_offered_the_msi_and_never_installs_it() {
    let world = World::new(
        "agent-v0.9.0",
        Plan::LinkOnly {
            asset: None,
            why: String::new(),
        },
        "citadel-agent 0.9.0",
    );
    let facts = host(
        Os::Windows,
        r"C:\Program Files\Citadel Agent\citadel-agent.exe",
    );
    let (plan, installer) = selected(&world, &facts);
    let engine = world.engine_for(plan, installer);
    engine.check().await;
    let update = engine.status(None).available.expect("an update is offered");
    assert!(
        !update.ready,
        "Windows is notify-only: the MSI needs elevation"
    );
    assert!(update
        .download_url
        .ends_with("/agent-v0.9.0/Citadel-Agent-x64.msi"));
    assert_eq!(
        world.source.downloads(),
        0,
        "nothing is downloaded for a link"
    );
    assert!(engine.consider(Trigger::UserAsked).await.is_err());
    world.sessions.set(0);
    engine.consider(Trigger::Idle).await.unwrap();
    assert!(world.installed().is_empty());
}

#[tokio::test]
async fn a_dpkg_install_is_offered_the_deb_and_never_installs_it() {
    let world = World::new(
        "agent-v0.9.0",
        Plan::LinkOnly {
            asset: None,
            why: String::new(),
        },
        "citadel-agent 0.9.0",
    );
    let mut facts = host(Os::Linux, "/usr/bin/citadel-agent");
    facts.dpkg_owned = true;
    let (plan, installer) = selected(&world, &facts);
    let engine = world.engine_for(plan, installer);
    engine.check().await;
    let update = engine.status(None).available.expect("an update is offered");
    assert!(!update.ready);
    assert!(update
        .download_url
        .ends_with("/agent-v0.9.0/citadel-agent-linux-x64.deb"));
    assert!(engine.consider(Trigger::UserAsked).await.is_err());
}

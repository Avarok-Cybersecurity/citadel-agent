//! The updater engine end to end against a fake release server: a fixture `latest` and fixture
//! assets served through the `ReleaseSource` seam, the real staging directory (real tar, a real
//! `--version` run of the staged file), and recording fakes for what would restart the agent.
//!
//! Mocks, and why: the release source (no test may reach GitHub), the attestation verifier
//! (Sigstore cannot be made to sign fixture bytes; the real one is tested against a real
//! recorded bundle in updater/attest_tests.rs), the installer (the real ones replace the
//! running executable and exit), and the announcer / sessions / settings (the service's).

// Shared with tests/updater_platforms.rs, which uses parts of it this file does not.
#[allow(dead_code)]
#[path = "updater_fakes/mod.rs"]
mod fakes;

use citadel_internal_service::updater::platform::{Method, Plan};
use citadel_internal_service::updater::policy::Trigger;
use fakes::*;

const TARBALL: &str = "citadel-agent-linux-x64.tar.gz";

fn tarball_plan() -> Plan {
    Plan::Apply {
        asset: TARBALL,
        method: Method::Tarball,
    }
}

#[tokio::test]
async fn a_verified_newer_release_is_staged_announced_and_installed_when_nobody_is_signed_in() {
    let world = World::new("agent-v0.9.0", tarball_plan(), "citadel-agent 0.9.0");
    world.engine.check().await;
    let announced = world.announced();
    assert_eq!(announced.len(), 1);
    let update = &announced[0];
    assert_eq!(
        (update.current.as_str(), update.latest.as_str()),
        ("0.8.8", "0.9.0")
    );
    assert!(update.ready, "{:?}", world.engine.status(None));
    assert!(update
        .download_url
        .ends_with("/agent-v0.9.0/citadel-agent-linux-x64.tar.gz"));
    assert!(update.notes_url.ends_with("/releases/tag/agent-v0.9.0"));

    world.sessions.set(1);
    world.engine.consider(Trigger::Idle).await.unwrap();
    assert!(
        world.installed().is_empty(),
        "installed with an account signed in"
    );

    world.sessions.set(0);
    world.engine.consider(Trigger::Idle).await.unwrap();
    let installed = world.installed();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].1, "0.9.0");
    assert_eq!(
        std::fs::read_to_string(&installed[0].0).unwrap(),
        world.agent_script,
        "the staged file is the tarball's binary"
    );
}

#[tokio::test]
async fn the_user_installs_with_accounts_open_but_never_automatically_with_auto_off() {
    let world = World::new("agent-v0.9.0", tarball_plan(), "citadel-agent 0.9.0");
    world.engine.set_auto_install(false).await.unwrap();
    world.engine.check().await;
    world.engine.consider(Trigger::Idle).await.unwrap();
    assert!(world.installed().is_empty());
    world.sessions.set(2);
    world.engine.consider(Trigger::UserAsked).await.unwrap();
    assert_eq!(world.installed().len(), 1);
}

#[tokio::test]
async fn a_checksum_mismatch_is_refused_and_nothing_is_installed_or_run() {
    let mut world = World::new("agent-v0.9.0", tarball_plan(), "citadel-agent 0.9.0");
    world.source.wrong_checksum(TARBALL);
    world.engine = world.rebuild();
    world.engine.check().await;
    let update = &world.announced()[0];
    assert!(!update.ready);
    assert!(
        update.notes_url == update.download_url,
        "a tampered file is not linked"
    );
    let status = world.engine.status(None);
    assert!(status.last_error.unwrap().contains("sha256"));
    assert_eq!(world.source.attestation_queries(), 0);
    assert!(world.engine.consider(Trigger::UserAsked).await.is_err());
    assert!(world.installed().is_empty());
    assert!(
        !world.staged_dir_exists("0.9.0"),
        "the refused download is removed"
    );
}

#[tokio::test]
async fn a_downgrade_or_the_same_version_is_never_offered_or_downloaded() {
    for tag in ["agent-v0.8.7", "agent-v0.8.8"] {
        let world = World::new(tag, tarball_plan(), "citadel-agent 0.8.7");
        world.engine.check().await;
        assert!(world.announced().is_empty(), "{tag}");
        assert_eq!(world.source.downloads(), 0, "{tag}");
        assert!(world.engine.status(None).available.is_none());
    }
}

#[tokio::test]
async fn a_release_missing_this_platforms_asset_offers_the_page_and_installs_nothing() {
    let mut world = World::new("agent-v0.9.0", tarball_plan(), "citadel-agent 0.9.0");
    world.source.drop_asset(TARBALL);
    world.engine = world.rebuild();
    world.engine.check().await;
    let update = &world.announced()[0];
    assert!(!update.ready);
    assert!(world
        .engine
        .status(None)
        .last_error
        .unwrap()
        .contains("has no asset"));
    assert_eq!(world.source.downloads(), 0);
}

#[tokio::test]
async fn a_network_error_is_reported_and_retried_at_the_next_check() {
    let world = World::new("agent-v0.9.0", tarball_plan(), "citadel-agent 0.9.0");
    world.source.fail_latest(true);
    world.engine.check().await;
    let status = world.engine.status(None);
    assert!(status.last_error.unwrap().contains("check failed"));
    assert!(status.available.is_none() && status.last_checked.is_some());

    world.source.fail_latest(false);
    world.source.fail_downloads(true);
    world.engine.check().await;
    assert!(!world.announced()[0].ready);

    world.source.fail_downloads(false);
    world.engine.check().await;
    assert!(world.announced()[1].ready, "a transient failure is retried");
}

#[tokio::test]
async fn an_unchanged_release_is_not_downloaded_again() {
    let world = World::new("agent-v0.9.0", tarball_plan(), "citadel-agent 0.9.0");
    world.engine.check().await;
    world.engine.check().await;
    assert_eq!(world.source.not_modified(), 1, "the ETag was sent back");
    assert_eq!(world.source.downloads(), 1);
    assert_eq!(
        world.announced().len(),
        2,
        "still announced to windows that joined since"
    );
}

#[tokio::test]
async fn a_staged_agent_reporting_another_version_is_refused() {
    let world = World::new("agent-v0.9.0", tarball_plan(), "citadel-agent 0.8.6");
    world.engine.check().await;
    assert!(!world.announced()[0].ready);
    assert!(world
        .engine
        .status(None)
        .last_error
        .unwrap()
        .contains("0.8.6"));
}

#[tokio::test]
async fn without_an_attestation_a_tarball_is_offered_as_a_download() {
    let mut world = World::new("agent-v0.9.0", tarball_plan(), "citadel-agent 0.9.0");
    world.source.no_attestations();
    world.engine = world.rebuild();
    world.engine.check().await;
    let update = &world.announced()[0];
    assert!(!update.ready);
    assert!(
        update.download_url.ends_with(TARBALL),
        "the file itself is fine to link"
    );
}

#[tokio::test]
async fn a_version_that_failed_to_install_is_not_retried_automatically() {
    let world = World::new("agent-v0.9.0", tarball_plan(), "citadel-agent 0.9.0");
    world.engine.check().await;
    world
        .engine
        .install_failed("0.9.0", "the new agent did not answer");
    world.engine.consider(Trigger::Idle).await.unwrap();
    assert!(world.installed().is_empty());
    world.engine.consider(Trigger::UserAsked).await.unwrap();
    assert_eq!(world.installed().len(), 1, "the user may still try it");
}

#[tokio::test]
async fn a_link_only_install_downloads_nothing_and_links_its_asset() {
    let plan = Plan::LinkOnly {
        asset: Some("citadel-agent-linux-x64.deb"),
        why: "installed with dpkg".to_string(),
    };
    let world = World::new("agent-v0.9.0", plan, "citadel-agent 0.9.0");
    world.engine.check().await;
    let update = &world.announced()[0];
    assert!(!update.ready);
    assert!(update
        .download_url
        .ends_with("/agent-v0.9.0/citadel-agent-linux-x64.deb"));
    assert_eq!(world.source.downloads(), 0);
}

#[tokio::test]
async fn read_rather_than_run_the_staged_version_installs_and_refuses_alike() {
    // The engine on hosts that cannot run the fixture script (Windows), exercised everywhere.
    let world = World::reading_versions("agent-v0.9.0", tarball_plan(), "citadel-agent 0.9.0");
    world.engine.check().await;
    assert!(
        world.announced()[0].ready,
        "{:?}",
        world.engine.status(None)
    );
    world.engine.consider(Trigger::Idle).await.unwrap();
    assert_eq!(world.installed().len(), 1);

    let world = World::reading_versions("agent-v0.9.0", tarball_plan(), "citadel-agent 0.8.6");
    world.engine.check().await;
    assert!(!world.announced()[0].ready);
    assert!(world
        .engine
        .status(None)
        .last_error
        .unwrap()
        .contains("0.8.6"));
}

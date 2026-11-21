//! A tarball's agent, started with a command that lacks a setting the new release requires,
//! updates itself: the release's signed `citadel-agent-launch.json` supplies what the command
//! lacks, the staged agent is asked `--check-launch` with the merged command, and the watchdog
//! starts the new version with it and would put the old one back with its own.
//!
//! The real `SelfReplace` and the real staging (tar, `--version`) run; the release source and
//! the attestation verifier are the fakes tests/updater_engine.rs justifies. The staged and the
//! installed "agents" are shell scripts that record how they were run.
#![cfg(unix)]

#[allow(dead_code)]
#[path = "updater_fakes/mod.rs"]
mod fakes;

use citadel_internal_service::updater::launch_settings::ASSET_NAME;
use citadel_internal_service::updater::platform::{Method, Plan};
use citadel_internal_service::updater::policy::Trigger;
use citadel_internal_service::updater::swap::SelfReplace;
use citadel_internal_service::updater::watchdog::WatchPlan;
use citadel_release_signature::SIGNATURE_SUFFIX;
use fakes::*;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const OLD_ARGS: [&str; 4] = ["--bind", "127.0.0.1:12345", "--stun-servers", "a:1,b:2,c:3"];
const MERGED: &str =
    "--bind 127.0.0.1:12345 --stun-servers a:1,b:2,c:3 --identity-unlock-idle-secs 900";

fn plan() -> Plan {
    Plan::Apply {
        asset: "citadel-agent-linux-x64.tar.gz",
        method: Method::Tarball,
    }
}

/// The new version: records each run's arguments in `<dir>/runs`, prints its version, and
/// (when `refuses` is set) fails `--check-launch` whenever that flag is in its command.
fn new_agent(dir: &Path, refuses: Option<&str>) -> String {
    let refusal = refuses.map_or(String::new(), |flag| {
        format!("case \"$*\" in *{flag}*) echo 'unknown {flag}' >&2; exit 1 ;; esac\n")
    });
    format!(
        "#!/bin/sh\necho \"$*\" >> '{}'\n{refusal}echo 'citadel-agent 0.9.0'\n",
        dir.join("runs").display()
    )
}

struct Agent {
    world: World,
    installed: PathBuf,
    installer: Arc<SelfReplace>,
    /// Where the new version records its runs.
    runs: tempfile::TempDir,
}

fn agent(args: &[&str], refuses: Option<&str>) -> Agent {
    let runs = tempfile::tempdir().unwrap();
    let script = new_agent(runs.path(), refuses);
    let world = World::with_script("agent-v0.9.0", plan(), &script, true);
    // The running (old) agent, which is started as the watchdog: it writes down its plan.
    let installed = world.dir().join("installed-citadel-agent");
    let old = format!(
        "#!/bin/sh\nprintf '%s' \"$CITADEL_UPDATE_WATCHDOG\" > '{}.tmp' && mv '{0}.tmp' '{0}'\n",
        world.dir().join("plan.json").display()
    );
    std::fs::write(&installed, old).unwrap();
    std::fs::set_permissions(&installed, std::fs::Permissions::from_mode(0o755)).unwrap();
    let installer = Arc::new(SelfReplace::new(
        installed.clone(),
        args.iter().map(|a| a.to_string()).collect(),
        "127.0.0.1:12345".parse().unwrap(),
        world.dir().join("cache"),
        || {},
    ));
    Agent {
        world,
        installed,
        installer,
        runs,
    }
}

impl Agent {
    fn engine(&self) -> citadel_internal_service::updater::engine::Engine {
        self.world.engine_for(plan(), self.installer.clone())
    }
    fn launch_checks(&self) -> Vec<String> {
        std::fs::read_to_string(self.runs.path().join("runs"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.strip_prefix("--check-launch "))
            .map(str::to_string)
            .collect()
    }
    /// The plan the watchdog was started with, once it has written it.
    fn watch_plan(&self) -> WatchPlan {
        let path = self.world.dir().join("plan.json");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !path.exists() {
            assert!(Instant::now() < deadline, "the watchdog was never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }
    fn untouched(&self) -> bool {
        std::fs::read_to_string(&self.installed)
            .unwrap()
            .contains("CITADEL_UPDATE_WATCHDOG")
    }
}

#[tokio::test]
async fn an_agent_started_without_the_new_setting_updates_with_it_added() {
    let agent = agent(&OLD_ARGS, None);
    let engine = agent.engine();
    engine.check().await;
    let update = engine.status(None).available.expect("0.9.0 is offered");
    assert!(update.ready, "{:?}", engine.status(None));
    assert_eq!(agent.launch_checks(), [MERGED]);

    engine.consider(Trigger::Idle).await.unwrap();
    let plan = agent.watch_plan();
    assert_eq!(
        plan.args.join(" "),
        MERGED,
        "the new version gets the setting"
    );
    assert_eq!(
        plan.restore_args, OLD_ARGS,
        "the old one comes back with its own command"
    );
    assert!(!agent.untouched(), "the new version is in place");
}

#[tokio::test]
async fn a_value_the_operator_set_is_kept() {
    let args = [
        "--bind",
        "127.0.0.1:12345",
        "--identity-unlock-idle-secs=60",
    ];
    let agent = agent(&args, None);
    let engine = agent.engine();
    engine.check().await;
    assert_eq!(agent.launch_checks(), [args.join(" ")]);
    engine.consider(Trigger::Idle).await.unwrap();
    assert_eq!(agent.watch_plan().args, args);
}

/// Nothing ran with the release's settings, nothing was installed, and the member is told why.
async fn refused(agent: &Agent, says: &str) {
    let engine = agent.engine();
    engine.check().await;
    let status = engine.status(None);
    let why = status.last_error.clone().unwrap_or_default();
    assert!(why.contains(says), "{why}");
    assert!(
        !status.available.as_ref().is_some_and(|u| u.ready),
        "{status:?}"
    );
    assert!(engine.consider(Trigger::UserAsked).await.is_err());
    assert!(agent.launch_checks().is_empty() && agent.untouched());
}

#[tokio::test]
async fn an_unsigned_launch_asset_is_refused() {
    let agent = agent(&OLD_ARGS, None);
    agent
        .world
        .source
        .drop_asset(&format!("{ASSET_NAME}{SIGNATURE_SUFFIX}"));
    refused(&agent, "has no citadel-agent-launch.json.mldsa.sig").await;
}

#[tokio::test]
async fn a_release_without_the_launch_asset_is_refused() {
    let agent = agent(&OLD_ARGS, None);
    agent.world.source.drop_asset(ASSET_NAME);
    refused(&agent, "has no citadel-agent-launch.json,").await;
}

#[tokio::test]
async fn a_launch_asset_signed_by_another_key_is_refused() {
    let agent = agent(&OLD_ARGS, None);
    agent
        .world
        .source
        .publish_signed(&[0x11; 32], ASSET_NAME, LAUNCH.as_bytes());
    refused(&agent, "ML-DSA: citadel-agent-launch.json").await;
}

#[tokio::test]
async fn a_tampered_value_is_refused() {
    let agent = agent(&OLD_ARGS, None);
    let tampered = LAUNCH.replace("900", "999");
    agent.world.source.replace(ASSET_NAME, tampered.as_bytes());
    refused(&agent, "ML-DSA: citadel-agent-launch.json").await;
}

#[tokio::test]
async fn a_signed_asset_in_an_unknown_format_is_refused() {
    let agent = agent(&OLD_ARGS, None);
    let v2 = r#"{"v":2,"settings":{"--identity-unlock-idle-secs":"900"}}"#;
    agent
        .world
        .source
        .publish_signed(&RELEASE_SEED, ASSET_NAME, v2.as_bytes());
    refused(&agent, "format 2 is not the 1 this agent reads").await;
}

#[tokio::test]
async fn a_new_version_that_refuses_the_merged_command_is_never_installed() {
    let agent = agent(&OLD_ARGS, Some("--identity-unlock-idle-secs"));
    let engine = agent.engine();
    engine.check().await;
    assert!(!engine.status(None).available.unwrap().ready);
    assert_eq!(
        agent.launch_checks(),
        [MERGED],
        "it was asked with the merged command"
    );
    engine.consider(Trigger::Idle).await.unwrap();
    let why = engine.consider(Trigger::UserAsked).await.unwrap_err();
    assert!(why.contains("unknown --identity-unlock-idle-secs"), "{why}");
    assert!(agent.untouched());
    assert!(!agent.world.dir().join("plan.json").exists());
}

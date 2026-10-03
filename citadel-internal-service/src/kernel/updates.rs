//! The agent's side of its updater (crate::updater): the seams only the running service can
//! fill — every window, the menu-bar app's stream, the sessions, the preferences store — and the
//! updater's life inside the service's.

use crate::kernel::conversations::ConversationIo;
use crate::kernel::notices::NoticeHub;
use crate::kernel::session_route::{deliver, Clients};
use crate::kernel::CitadelWorkspaceService;
use crate::updater::attest::SigstoreVerifier;
use crate::updater::engine::{Engine, LOG_TARGET};
use crate::updater::github::GitHub;
use crate::updater::io::{Announcer, Installer, Io, Sessions, SettingsStore};
use crate::updater::platform::{install_kind, plan};
use crate::updater::staging::FsStaging;
use crate::updater::{host, installer_for, version, UpdaterConfig};
use async_trait::async_trait;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{InternalServiceResponse, UpdateAvailable, UpdateInstall};
use citadel_sdk::logging::{error, info};
use citadel_sdk::prelude::Ratchet;
use semver::Version;
use std::path::Path;
use std::sync::{Arc, OnceLock};

pub const AUTO_INSTALL_KEY: &str = "agent_update_auto_install";

/// The configuration the shipped binary gave, and the engine once the service has started.
#[derive(Default)]
pub struct UpdatesSlot {
    pub(crate) config: Option<UpdaterConfig>,
    pub(crate) engine: OnceLock<Arc<Engine>>,
}

impl<T, R: Ratchet> CitadelWorkspaceService<T, R> {
    /// Check GitHub for newer agent releases, and install them as `config` allows.
    pub fn with_updater(mut self, config: UpdaterConfig) -> Self {
        self.updates = Arc::new(UpdatesSlot {
            config: Some(config),
            engine: OnceLock::new(),
        });
        self
    }
}

/// Runs the updater for the life of the service; pends forever when none is configured or it
/// could not start (it says why).
pub(crate) async fn run<T: IOInterface + Sync, R: Ratchet>(this: CitadelWorkspaceService<T, R>) {
    let Some(config) = this.updates.config.clone() else {
        return std::future::pending().await;
    };
    match engine_for(&this, &config) {
        Ok(engine) => {
            let engine = this.updates.engine.get_or_init(|| engine).clone();
            crate::updater::run(engine).await
        }
        Err(e) => {
            error!(target: LOG_TARGET, "the updater is off: {e}");
            std::future::pending().await
        }
    }
}

fn engine_for<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    config: &UpdaterConfig,
) -> Result<Arc<Engine>, String> {
    let current = version::running(&config.current_version)?;
    let facts = host::gather(config.launched_by_app)?;
    let kind = install_kind(&facts);
    let plan = plan(&kind, facts.os, facts.arch);
    info!(target: LOG_TARGET, "running {current} as {kind:?}: {plan:?}");
    let mac_app: Arc<dyn Installer> = Arc::new(MacAppHandoff(this.notices.clone()));
    let io = Io {
        source: Arc::new(GitHub::new(&config.current_version)?),
        verifier: Arc::new(SigstoreVerifier::new()?),
        staging: Arc::new(FsStaging::new(config.cache_dir.clone())),
        installer: installer_for(config, &kind, &plan, mac_app),
        announcer: Arc::new(Everyone(this.tx_to_localhost_clients.clone())),
        settings: Arc::new(KvSettings(Box::new(this.clone()))),
        sessions: Arc::new(MapSessions(this.server_connection_map.clone())),
        now: unix_now,
    };
    Ok(Arc::new(Engine::new(current, plan, io)))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Every window, signed in or not: an update is the agent's, not a session's.
struct Everyone(Clients);

impl Announcer for Everyone {
    fn announce(&self, update: &UpdateAvailable) {
        let all: Vec<_> = self.0.read().keys().copied().collect();
        deliver(
            &self.0,
            &all,
            InternalServiceResponse::UpdateAvailable(update.clone()),
        );
    }
}

/// The menu-bar app owns the bundle, so it swaps it; the agent hands it the verified image.
struct MacAppHandoff(Arc<NoticeHub>);

impl Installer for MacAppHandoff {
    fn can_install(&self) -> Result<(), String> {
        if self.0.has_stream() {
            Ok(())
        } else {
            Err("Citadel Agent.app is not listening, so it cannot install the update".to_string())
        }
    }

    fn install(&self, staged: &Path, version: &Version) -> Result<(), String> {
        let handoff = InternalServiceResponse::UpdateInstall(UpdateInstall {
            cid: 0,
            version: version.to_string(),
            path: staged.to_string_lossy().into_owned(),
            request_id: None,
        });
        match self.0.send_to_stream(handoff) {
            0 => self.can_install(),
            _ => Ok(()),
        }
    }
}

struct MapSessions<R: Ratchet>(
    Arc<parking_lot::RwLock<std::collections::HashMap<u64, crate::kernel::Connection<R>>>>,
);

impl<R: Ratchet> Sessions for MapSessions<R> {
    fn signed_in(&self) -> usize {
        self.0.read().len()
    }
}

/// The setting in the agent's key-value store, beside the per-account mutes.
struct KvSettings(Box<dyn ConversationIo>);

#[async_trait]
impl SettingsStore for KvSettings {
    async fn auto_install(&self) -> Result<Option<bool>, String> {
        let value = self.0.kv().get(AUTO_INSTALL_KEY).await?;
        Ok(value.and_then(|v| v.first().map(|b| *b == 1)))
    }

    async fn set_auto_install(&self, on: bool) -> Result<(), String> {
        self.0.kv().set(AUTO_INSTALL_KEY, vec![u8::from(on)]).await
    }
}

#[cfg(test)]
#[path = "updates_tests.rs"]
mod tests;

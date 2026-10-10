//! What the engine says about itself, and the changes made to it from outside a check.

use super::engine::{Engine, Offer, AUTO_INSTALL_UNSET, LOG_TARGET};
use citadel_internal_service_types::{UpdateAvailable, UpdateStatus};
use citadel_sdk::logging::error;
use semver::Version;
use uuid::Uuid;

impl Engine {
    pub async fn set_auto_install(&self, on: bool) -> Result<(), String> {
        self.io.settings.set_auto_install(on).await?;
        self.state.lock().auto_install = Some(on);
        Ok(())
    }

    /// The menu-bar app could not install `version`: the agent that hears this is still the old
    /// one, so record why and never retry it automatically.
    pub fn install_failed(&self, version: &str, why: &str) {
        error!(target: LOG_TARGET, "the menu-bar app could not install {version}: {why}");
        if let Ok(version) = Version::parse(version) {
            self.io.staging.mark_failed(&version);
        }
        let offer = {
            let mut state = self.state.lock();
            state.last_error = Some(format!("Installing {version} failed: {why}"));
            state.offer.clone()
        };
        if let Some(offer) = offer {
            self.io.announcer.announce(&self.available(&offer));
        }
    }

    pub fn status(&self, request_id: Option<Uuid>) -> UpdateStatus {
        let state = self.state.lock();
        UpdateStatus {
            cid: 0,
            current: self.current.to_string(),
            available: state.offer.as_ref().map(|o| self.available(o)),
            auto_install: state.auto_install.unwrap_or(AUTO_INSTALL_UNSET),
            last_checked: state.last_checked,
            last_error: state.last_error.clone(),
            request_id,
        }
    }

    pub(super) fn available(&self, offer: &Offer) -> UpdateAvailable {
        UpdateAvailable {
            cid: 0,
            current: self.current.to_string(),
            latest: offer.version.to_string(),
            notes_url: offer.notes_url.clone(),
            download_url: offer.download_url.clone(),
            ready: offer.staged.is_some() && self.io.installer.can_install().is_ok(),
            // Nothing is staged before its signature verified (fetch.rs).
            mldsa_verified: offer.staged.is_some(),
            request_id: None,
        }
    }
}

//! The menu-bar app's half of a macOS install, and how the agent learns it never happened.
//!
//! The app owns `Citadel Agent.app`, so the agent hands it the verified disk image and the app
//! swaps the bundle. Taking it means stopping this agent: the app mounts and verifies the image
//! (a few seconds), then stops the agent before the swap. So an agent still running well after
//! the handoff knows the app did not take it, whatever the reason: its notice stream stopped
//! reading (found live 2026-11-18: 31,650 bytes queued unread on the 0.11.0 app's socket, and
//! no update since 0.11.0 was ever installed), it crashed, or it refused without reporting.
//! Before this, the handoff was fire-and-forget: the request that asked for the install was
//! answered as a success, the window waited for a restart that never came, and nothing anywhere
//! recorded why.

use crate::kernel::notices::NoticeHub;
use crate::updater::io::{Installer, Staged};
use citadel_internal_service_types::{InternalServiceResponse, UpdateInstall};
use semver::Version;
use std::sync::Arc;
use std::time::Duration;

/// How long the app has, after the handoff, to stop this agent. It mounts, copies and verifies
/// the image first: 3.9 s for 0.12.1's on the owner's Mac (2026-11-18), so this is generous.
pub const HANDOFF_TAKEN_WITHIN: Duration = Duration::from_secs(60);

/// Why there is nothing to hand the image to.
pub const NOT_LISTENING: &str =
    "Citadel Agent.app is not listening, so it cannot install the update";

/// What is recorded when this agent, at `current`, is still running `within` after handing
/// `version` to the app (pure).
pub fn not_taken(version: &Version, current: &str, within: Duration) -> String {
    format!(
        "Citadel Agent.app did not start installing {version} within {}s of being handed it, so \
         this agent is still {current}. Download {version} and open it to install it.",
        within.as_secs()
    )
}

/// Called with the version the app did not take.
pub type OnNotTaken = Arc<dyn Fn(&Version) + Send + Sync>;

/// The menu-bar app's installer: hands it the image, then watches for being stopped.
pub struct MacAppHandoff {
    hub: Arc<NoticeHub>,
    within: Duration,
    on_not_taken: OnNotTaken,
}

impl MacAppHandoff {
    pub(crate) fn new(hub: Arc<NoticeHub>, within: Duration, on_not_taken: OnNotTaken) -> Self {
        Self {
            hub,
            within,
            on_not_taken,
        }
    }
}

impl Installer for MacAppHandoff {
    fn can_install(&self, _staged: &Staged) -> Result<(), String> {
        if self.hub.has_stream() {
            Ok(())
        } else {
            Err(NOT_LISTENING.to_string())
        }
    }

    /// Ok once the app's stream has the handoff. Delivered to nobody is a failure even while a
    /// subscriber is still listed: listed is not reached (the old fallback answered Ok there).
    fn install(&self, staged: &Staged, version: &Version) -> Result<(), String> {
        let handoff = InternalServiceResponse::UpdateInstall(UpdateInstall {
            cid: 0,
            version: version.to_string(),
            path: staged.path.to_string_lossy().into_owned(),
            request_id: None,
        });
        if self.hub.send_to_stream(handoff) == 0 {
            return Err(NOT_LISTENING.to_string());
        }
        let (within, on_not_taken, version) =
            (self.within, self.on_not_taken.clone(), version.clone());
        // Taking it stops this process, and the timer with it: firing means it was not taken.
        drop(tokio::spawn(async move {
            tokio::time::sleep(within).await;
            on_not_taken(&version);
        }));
        Ok(())
    }
}

#[cfg(test)]
#[path = "updates_handoff_tests.rs"]
mod tests;

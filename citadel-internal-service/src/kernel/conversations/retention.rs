//! Expiring old messages: the account's one sweeper.
//!
//! The web UI swept every in-memory conversation hourly from each tab. The
//! agent replaces it (mw5 removes the UI's): it sweeps every conversation of
//! every hosted account, hourly and whenever retention settings change.

use super::engine::{preferences, store, Change, Engine};
use super::io::ConversationIo;
use citadel_internal_service_types::{ConversationEventKind, Retention};
use citadel_sdk::logging::warn;

pub(crate) const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(60 * 60);
const DAY_MS: f64 = 24.0 * 60.0 * 60.0 * 1000.0;

impl Engine {
    pub(crate) async fn sweep(&self, io: &dyn ConversationIo, cid: u64) {
        let prefs = match preferences(io, cid).await {
            Ok(prefs) => prefs,
            Err(e) => {
                warn!(target: "citadel", "[CONVERSATIONS] {cid}: retention skipped, preferences unreadable: {e}");
                return;
            }
        };
        for setting in &prefs.retention {
            let Retention::Days(days) = setting.retention else {
                continue;
            };
            let peer = setting.peer_cid;
            let now = io.now_ms();
            let cutoff = now - f64::from(days) * DAY_MS;
            let removed = {
                let _held = self.lock(cid, peer).await;
                store(io, cid).prune_older_than(peer, cutoff, now).await
            };
            match removed {
                Ok(0) => {}
                Ok(_) => {
                    let change = Change {
                        kind: ConversationEventKind::Expired,
                        message: None,
                        message_id: None,
                        metadata: None,
                        request_id: None,
                    };
                    self.announce(io, cid, peer, change).await;
                }
                Err(e) => {
                    warn!(target: "citadel", "[CONVERSATIONS] {cid}: retention for {peer} failed: {e}")
                }
            }
        }
    }
}

/// The one retention sweeper for every hosted account: runs for the life of the agent.
pub(crate) async fn sweeper<T, R>(this: crate::kernel::CitadelWorkspaceService<T, R>)
where
    T: citadel_internal_service_connector::io_interface::IOInterface + Sync,
    R: citadel_sdk::prelude::Ratchet,
{
    let mut every = tokio::time::interval(SWEEP_EVERY);
    loop {
        every.tick().await;
        for cid in this.ilm_hosts.hosted() {
            this.conversations.sweep(&this, cid).await;
        }
    }
}

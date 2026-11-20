//! What a RE-VFS transfer's end does to the account's index (index.rs): a completed store is
//! recorded, a completed take is forgotten. Each hook also holds the transfer's `UseToken`, so
//! the account's key stays as it is until the transfer ends (usage.rs), whether it completes
//! or not: a hook never run is dropped with the transfer's stream. A re-key that could not start
//! while the transfer held the key is started once it lets go (node.rs `resume`).

use super::index::Location;
use crate::kernel::revfs_correlation::PullUse;
use crate::kernel::send_correlation::PendingStore;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::Ratchet;

pub(crate) type Hook = Box<dyn FnOnce() + Send>;

/// Records the store once it completed.
pub(crate) fn on_stored<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    store: Option<PendingStore>,
) -> Option<Hook> {
    let PendingStore { entry, token } = store?;
    let this = this.clone();
    Some(Box::new(move || {
        drop(crate::telemetry::span::spawn_in_current_span(async move {
            let recorded =
                super::record_store(this.revfs_vault.get(), this.remote(), cid, entry).await;
            if let Err(err) = recorded {
                warn!(target: "citadel", "[RE-VFS] A store of account {cid} completed but is not in its index: {err}");
            }
            drop(token);
            super::resume(&this, cid);
        }));
    }))
}

/// Holds a pull's token until its transfer ends, and forgets a take once it completed.
pub(crate) fn on_pulled<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    pull: PullUse,
) -> Option<Hook> {
    let PullUse { token, taken } = pull;
    if token.is_none() && taken.is_none() {
        return None;
    }
    let this = this.clone();
    Some(Box::new(move || {
        drop(crate::telemetry::span::spawn_in_current_span(async move {
            if let Some((location, path)) = taken {
                forget(&this, cid, location, &path).await;
            }
            drop(token);
            super::resume(&this, cid);
        }));
    }))
}

async fn forget<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    location: Location,
    path: &str,
) {
    let forgotten = super::forget(this.revfs_vault.get(), this.remote(), cid, location, path).await;
    if let Err(err) = forgotten {
        warn!(target: "citadel", "[RE-VFS] An object of account {cid} was taken but is still in its index: {err}");
    }
}

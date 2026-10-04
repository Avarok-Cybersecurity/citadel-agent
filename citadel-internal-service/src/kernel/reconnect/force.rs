//! Bringing back a session whose SDK side was just abandoned (the supervisor found its path
//! dead). It is the recovered-drop flow: the same marks, the same reconnect run, so the
//! credentials, the ILM, the supervisor and the windows are all kept.

use super::task::{self, Began};
use super::LOG_TARGET;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::Ratchet;

/// A no-op when the drop was already reported (the abandon's own disconnect event gets
/// there first) or the session is being ended.
pub(crate) fn restart<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
) {
    let Began::Reconnecting { lost_peers } = task::begin(&this.server_connection_map, cid) else {
        info!(target: LOG_TARGET, "[Reconnect] {cid}: already reconnecting, or not a live link");
        return;
    };
    this.prune_dropped_link_state(cid);
    if let Err(err) = task::spawn(this, cid, &lost_peers) {
        warn!(target: LOG_TARGET, "[Reconnect] {cid}: could not start the reconnect: {err:?}");
    }
}

//! The SDK side of a reconnect: mark the session, tell the UI, and start the run that
//! retries (reconnect/run.rs). Every decision is policy.rs's.

use super::instance::{self, Instance};
use super::lost_peers;
use super::policy::DropAction;
use super::report::notify;
use super::run::run;
use super::LinkState;
use crate::kernel::supervisor::{LinkStatus, Signal};
use crate::kernel::{group_channels, CitadelWorkspaceService, Connection};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{InternalServiceResponse, ServerConnectionLost};
use citadel_sdk::prelude::{NetworkError, Ratchet};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// What an unrequested drop did to the session.
pub(crate) enum Began {
    /// Kept and marked; the caller notifies (of `lost_peers` too) and spawns the reconnect.
    Reconnecting {
        lost_peers: Vec<u64>,
    },
    AlreadyReconnecting,
    /// The report is of an SDK session this entry no longer is (reconnect/instance.rs).
    NotThisLink,
    /// Being ended by its user: the caller removes it as it always did.
    Remove,
    /// Not in the map: nothing to keep.
    NotTracked,
}

/// Decide, and if it is a reconnect, mark the session before anything else can see it
/// `Up`. Its peers, groups and C2S transfers belonged to the dead link and go. `reported`
/// is the instance the drop report names, if it names one.
pub(crate) fn begin<R: Ratchet>(
    map: &Arc<RwLock<HashMap<u64, Connection<R>>>>,
    cid: u64,
    reported: Option<Instance>,
) -> Began {
    let mut lock = map.write();
    let Some(conn) = lock.get_mut(&cid) else {
        return Began::NotTracked;
    };
    let Some(action) = instance::on_reported_drop(conn.link, conn.instance, reported) else {
        return Began::NotThisLink;
    };
    match action {
        DropAction::AlreadyReconnecting => Began::AlreadyReconnecting,
        DropAction::Remove => Began::Remove,
        DropAction::Reconnect => {
            conn.link = LinkState::Reconnecting;
            conn.handoff.next_generation();
            let lost_peers = lost_peers::take(&mut conn.peers);
            // Only the send halves live here, and dropping a channel sends nothing: the
            // member stays in every group (leaving is only `leave()`, in
            // requests/group/leave.rs), so the server still lists this session's groups:
            // it prompts the new session to rejoin those it joined and to re-found those
            // it owns, and `responses/group_channel_created.rs` adopts the channel each
            // one opens into this same (never removed) entry.
            conn.groups = group_channels::GroupChannels::new();
            conn.c2s_file_transfer_handlers.clear();
            Began::Reconnecting { lost_peers }
        }
    }
}

/// Tell the UI and start retrying. Called once, after `begin` answered `Reconnecting`.
pub(crate) fn spawn<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    lost_peers: &[u64],
) -> Result<(), NetworkError> {
    notify(
        this,
        cid,
        InternalServiceResponse::ServerConnectionLost(ServerConnectionLost {
            cid,
            reconnecting: true,
            request_id: None,
        }),
    )?;
    this.supervisors
        .signal(cid, Signal::Link(LinkStatus::Reconnecting));
    for notice in lost_peers::notices(cid, lost_peers) {
        notify(this, cid, notice)?;
    }
    let generation = {
        let lock = this.server_connection_map.read();
        lock.get(&cid).map(|conn| conn.handoff.generation())
    };
    if let Some(generation) = generation {
        resume(this, cid, generation);
    }
    Ok(())
}

/// Start reconnect run `generation` for `cid`, telling the UI nothing: it already knows
/// the session is reconnecting (a takeover that failed hands it back through here).
pub(crate) fn resume<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    generation: u64,
) {
    let this = this.clone();
    drop(crate::telemetry::span::spawn_in_span(
        crate::telemetry::span::session_span(cid),
        async move { run(&this, cid, generation).await },
    ));
}

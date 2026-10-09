//! What the UI is told about a reconnect, and the give-up that removes the session.

use super::{LinkState, LOG_TARGET};
use crate::kernel::session_route::SessionRoute;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    DisconnectNotification, InternalServiceResponse, ServerReconnectFailed,
};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{NetworkError, Ratchet};

/// The session is gone after all: say why, then what a removal always said.
pub(super) fn fail<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    generation: u64,
    reason: String,
) {
    // Checked and removed under one lock: a user Disconnect that got here first owns
    // the answer, and this says nothing; so does a sign-in taking the session over,
    // and a newer reconnect run.
    //
    // Recorded as signed out under the same lock, for a UI that is not attached to hear
    // the notification below. A sign-in inserts its session and clears the record under
    // this lock too (requests/connect.rs), so a record never outlives a session it
    // raced with.
    let removed = {
        let mut lock = this.server_connection_map.write();
        match lock.get(&cid) {
            Some(conn)
                if conn.link == LinkState::Reconnecting
                    && conn.handoff.generation() == generation =>
            {
                lock.remove(&cid).inspect(|conn| {
                    this.signed_out
                        .record(cid, conn.username.clone(), reason.clone())
                })
            }
            _ => None,
        }
    };
    let Some(removed) = removed else {
        return;
    };
    this.prune_cid_scoped_state(cid, None);
    let route = SessionRoute::new(
        removed.subscribers.clone(),
        this.tx_to_localhost_clients.clone(),
    );
    drop(removed);
    warn!(target: LOG_TARGET, "[Reconnect] gave up on {cid}: {reason}");
    for response in [
        InternalServiceResponse::ServerReconnectFailed(ServerReconnectFailed {
            cid,
            reason,
            request_id: None,
        }),
        InternalServiceResponse::DisconnectNotification(DisconnectNotification {
            cid,
            peer_cid: None,
            request_id: None,
            ended_locally: None,
        }),
    ] {
        if route.send(response).is_empty() {
            info!(target: LOG_TARGET, "[Reconnect] {cid}: no window attached to hear the give-up");
        }
    }
}

pub(super) fn notify<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    response: InternalServiceResponse,
) -> Result<(), NetworkError> {
    // Every attached window hears the link's state; nobody attached is not an error.
    if let Some(subscribers) = crate::kernel::membership::subscribers_of(this, cid) {
        SessionRoute::new(subscribers, this.tx_to_localhost_clients.clone()).send(response);
    }
    Ok(())
}

/// Nothing is left to hand these errors to; they are recorded, not dropped.
pub(super) fn logged(cid: u64, doing: &str, result: Result<(), NetworkError>) {
    if let Err(err) = result {
        warn!(target: LOG_TARGET, "[Reconnect] {cid}: {doing} failed: {err:?}");
    }
}

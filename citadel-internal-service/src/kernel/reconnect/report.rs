//! What the UI is told about a reconnect, and the give-up that removes the session.

use super::LinkState;
use crate::kernel::{send_response_to_tcp_client, CitadelWorkspaceService};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    DisconnectNotification, InternalServiceResponse, ServerReconnectFailed,
};
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::{NetworkError, Ratchet};
use std::sync::atomic::Ordering;

/// The session is gone after all: say why, then what a removal always said.
pub(super) fn fail<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    reason: String,
) {
    // Checked and removed under one lock: a user Disconnect that got here first owns
    // the answer, and this says nothing.
    let removed = {
        let mut lock = this.server_connection_map.write();
        match lock.get(&cid) {
            Some(conn) if conn.link == LinkState::Reconnecting => lock.remove(&cid),
            _ => None,
        }
    };
    let Some(removed) = removed else {
        return;
    };
    this.prune_cid_scoped_state(cid, None);
    let tcp_uuid = removed
        .associated_localhost_connection
        .load(Ordering::Relaxed);
    drop(removed);
    warn!(target: "citadel", "[Reconnect] gave up on {cid}: {reason}");
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
        }),
    ] {
        let sent = send_response_to_tcp_client(&this.tx_to_localhost_clients, response, tcp_uuid);
        logged(cid, "reporting the give-up", sent);
    }
}

pub(super) fn notify<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    response: InternalServiceResponse,
) -> Result<(), NetworkError> {
    let tcp_uuid = {
        let lock = this.server_connection_map.read();
        lock.get(&cid)
            .map(|conn| conn.associated_localhost_connection.load(Ordering::Relaxed))
    };
    match tcp_uuid {
        Some(tcp_uuid) => {
            send_response_to_tcp_client(&this.tx_to_localhost_clients, response, tcp_uuid)
        }
        None => Ok(()),
    }
}

/// Nothing is left to hand these errors to; they are recorded, not dropped.
pub(super) fn logged(cid: u64, doing: &str, result: Result<(), NetworkError>) {
    if let Err(err) = result {
        warn!(target: "citadel", "[Reconnect] {cid}: {doing} failed: {err:?}");
    }
}

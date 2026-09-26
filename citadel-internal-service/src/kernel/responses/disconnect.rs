//! C2S (Client-to-Server) Disconnect Response Handler
//!
//! This module handles SDK `NodeResult::Disconnect` events - inbound notifications
//! that a C2S connection has been terminated.
//!
//! ## SDK Event Flow (v0.13.1+)
//! - C2S disconnects: `NodeResult::Disconnect { conn_type: ClientConnectionType::Server }`
//! - P2P disconnects: `NodeResult::PeerEvent { PeerSignal::Disconnect }` (handled in peer_event.rs)
//!
//! ## Design: a drop nobody asked for is reconnected
//! This used to remove the session on every report, so a server deploy (which resets its
//! WebSocket) signed every user out. Now an unrequested drop keeps the session under its
//! CID and reconnects it; see kernel/reconnect/mod.rs. The session is removed here only
//! while its user is ending it, and the reconnect removes it if it gives up.
//!
//! ## Distinction from Request Handler
//! - `requests/peer/disconnect.rs`: User-initiated (outbound) disconnect - calls SDK then cleans state
//! - `responses/disconnect.rs` (this file): SDK-initiated (inbound) C2S disconnect event

use crate::kernel::reconnect::task::{self, Began};
use crate::kernel::requests::peer::{cleanup_state, DisconnectedConnection};
use crate::kernel::{send_response_to_tcp_client, CitadelWorkspaceService};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{DisconnectNotification, InternalServiceResponse};
use citadel_sdk::prelude::{ClientConnectionType, Disconnect, NetworkError, Ratchet};

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    disconnect: Disconnect,
) -> Result<(), NetworkError> {
    // If disconnect is due to a rejected connection attempt, the existing session should remain valid.
    // These are cases where a duplicate/failed connection attempt was rejected,
    // but the original session is still active and shouldn't be removed.
    let rejected_connection_messages =
        ["Session Already Connected", "Preconnect signalled to halt"];

    if let Some(reject_msg) = rejected_connection_messages
        .iter()
        .find(|m| disconnect.message.contains(**m))
    {
        // The same text ends a LIVE session: after the SDK shut one down (a missing
        // ratchet, measured live), the server refused its reconnect with HALT, and this
        // kept the dead session as though it were the original -- never reconnected,
        // the page never told, every action silently going nowhere.
        let sdk_holds = match disconnect_cid(&disconnect) {
            Some(cid) => this.client_or_peer_in_protocol(cid, None).await,
            None => Ok(true),
        };
        if refusal_leaves_session_standing(&sdk_holds) {
            citadel_sdk::logging::info!(
                target: "citadel",
                "Disconnect due to '{}' - preserving existing session in server_connection_map",
                reject_msg
            );
            return Ok(());
        }
        citadel_sdk::logging::warn!(target: "citadel", "Disconnect due to '{reject_msg}', and the SDK no longer holds the session: treating it as dropped");
    }

    // In SDK v0.13.1+, NodeResult::Disconnect only carries C2S connection types.
    // P2P disconnects are now handled via NodeResult::PeerEvent { PeerSignal::Disconnect }.
    if let Some(conn) = disconnect.conn_type {
        let cid = match conn {
            ClientConnectionType::Server { session_cid } => session_cid,
            ClientConnectionType::Extended { session_cid, .. } => session_cid,
        };

        citadel_sdk::logging::info!(
            target: "citadel",
            "[Disconnect Response] SDK reports C2S session {} disconnected. Reason: {}",
            cid,
            disconnect.message
        );

        match task::begin(&this.server_connection_map, cid) {
            Began::NotTracked => {}
            Began::AlreadyReconnecting => {
                citadel_sdk::logging::info!(target: "citadel", "[Disconnect Response] {cid} is already reconnecting");
            }
            Began::Reconnecting => {
                this.prune_cid_scoped_state(cid, None);
                return task::spawn(this, cid);
            }
            Began::Remove => return remove(this, cid),
        }
    } else {
        citadel_sdk::logging::warn!(target: "citadel", "The disconnect request does not contain a connection type")
    }

    Ok(())
}

fn disconnect_cid(disconnect: &Disconnect) -> Option<u64> {
    match disconnect.conn_type? {
        ClientConnectionType::Server { session_cid } => Some(session_cid),
        ClientConnectionType::Extended { session_cid, .. } => Some(session_cid),
    }
}

/// A refused connection attempt leaves the tracked session standing only while the
/// SDK still holds it. A failed query is not evidence it is gone, so it preserves.
pub(crate) fn refusal_leaves_session_standing(sdk_holds: &Result<bool, NetworkError>) -> bool {
    !matches!(sdk_holds, Ok(false))
}

/// The session was being ended by its user: mirror the SDK, as this always did.
fn remove<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
) -> Result<(), NetworkError> {
    // NOTE: SDK has already disconnected, so we don't call disconnect_removed.
    this.prune_cid_scoped_state(cid, None);
    let Some(disconnected) = cleanup_state(&this.server_connection_map, cid, None) else {
        return Ok(());
    };
    let tcp_uuid = match &disconnected {
        DisconnectedConnection::C2S { tcp_uuid, .. } => *tcp_uuid,
        DisconnectedConnection::P2P { tcp_uuid, .. } => *tcp_uuid,
    };
    drop(disconnected);
    let response = InternalServiceResponse::DisconnectNotification(DisconnectNotification {
        cid,
        peer_cid: None,
        request_id: None,
    });
    send_response_to_tcp_client(&this.tx_to_localhost_clients, response, tcp_uuid)
}

#[cfg(test)]
mod tests {
    use super::refusal_leaves_session_standing;
    use citadel_sdk::prelude::NetworkError;

    #[test]
    fn a_refusal_keeps_the_session_only_while_the_sdk_holds_it() {
        assert!(
            refusal_leaves_session_standing(&Ok(true)),
            "a rejected duplicate leaves the original"
        );
        assert!(
            !refusal_leaves_session_standing(&Ok(false)),
            "a session the SDK has ended is dropped, not preserved"
        );
        assert!(
            refusal_leaves_session_standing(&Err(NetworkError::msg("query failed"))),
            "a failed query is not evidence the session is gone"
        );
    }
}

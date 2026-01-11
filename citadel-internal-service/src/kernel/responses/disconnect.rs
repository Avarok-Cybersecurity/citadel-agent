//! C2S (Client-to-Server) Disconnect Response Handler
//!
//! This module handles SDK `NodeResult::Disconnect` events - inbound notifications
//! that a C2S connection has been terminated.
//!
//! ## SDK Event Flow
//! 1. SDK sends `DisconnectFromHypernode` to server
//! 2. Server terminates session, SDK receives `NodeResult::Disconnect`
//! 3. This handler checks if session is orphaned (no active TCP connection)
//! 4. If orphaned: preserve session for potential reconnection via ClaimSession
//! 5. If active: clean up internal service state and notify TCP client
//!
//! ## Distinction from Request Handler
//! - `requests/peer/disconnect.rs`: User-initiated (outbound) disconnect - calls SDK then cleans state
//! - `responses/disconnect.rs` (this file): SDK-initiated (inbound) event - preserves orphans, cleans active
//!
//! Both use the shared `cleanup_state()` function for DRY state management.

use crate::kernel::requests::peer::cleanup_state;
use crate::kernel::{send_response_to_tcp_client, CitadelWorkspaceService};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{DisconnectNotification, InternalServiceResponse};
use citadel_sdk::prelude::{Disconnect, NetworkError, Ratchet, VirtualTargetType};
use std::sync::atomic::Ordering;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    disconnect: Disconnect,
) -> Result<(), NetworkError> {
    // If disconnect is due to a rejected connection attempt, the existing session should remain valid.
    // These are cases where a duplicate/failed connection attempt was rejected,
    // but the original session is still active and shouldn't be removed.
    let rejected_connection_messages =
        ["Session Already Connected", "Preconnect signalled to halt"];

    for reject_msg in &rejected_connection_messages {
        if disconnect.message.contains(reject_msg) {
            citadel_sdk::logging::info!(
                target: "citadel",
                "Disconnect due to '{}' - preserving existing session in server_connection_map",
                reject_msg
            );
            return Ok(());
        }
    }

    if let Some(conn) = disconnect.v_conn_type {
        let (cid, peer_cid) = match conn {
            VirtualTargetType::LocalGroupServer { session_cid } => (session_cid, None),
            VirtualTargetType::LocalGroupPeer {
                session_cid,
                peer_cid,
            } => (session_cid, Some(peer_cid)),
            _ => return Ok(()),
        };

        // Check if this is an orphaned session (no active TCP connection)
        // Orphaned sessions should be preserved for potential reconnection via ClaimSession
        let is_orphaned = {
            let server_map = this.server_connection_map.read();
            if let Some(conn) = server_map.get(&cid) {
                let associated_tcp = conn.associated_localhost_connection.load(Ordering::Relaxed);
                let tcp_map = this.tx_to_localhost_clients.read();
                !tcp_map.contains_key(&associated_tcp)
            } else {
                // Session doesn't exist, nothing to preserve
                false
            }
        };

        if is_orphaned && peer_cid.is_none() {
            // This is an orphaned C2S session - preserve it for reconnection
            citadel_sdk::logging::info!(
                target: "citadel",
                "[Disconnect Response] Preserving orphaned session {} for potential reconnection",
                cid
            );
            return Ok(());
        }

        // Session has active TCP connection or is a P2P disconnect - proceed with cleanup
        if let Some(conn_uuid) = cleanup_state(&this.server_connection_map, cid, peer_cid) {
            let response = InternalServiceResponse::DisconnectNotification(DisconnectNotification {
                cid,
                peer_cid,
                request_id: None,
            });
            return send_response_to_tcp_client(&this.tx_to_localhost_clients, response, conn_uuid);
        }
    } else {
        citadel_sdk::logging::warn!(target: "citadel", "The disconnect request does not contain a connection type")
    }

    Ok(())
}

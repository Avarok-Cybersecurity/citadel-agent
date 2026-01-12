//! C2S (Client-to-Server) Disconnect Response Handler
//!
//! This module handles SDK `NodeResult::Disconnect` events - inbound notifications
//! that a C2S connection has been terminated.
//!
//! ## SDK Event Flow (v0.13.1+)
//! - C2S disconnects: `NodeResult::Disconnect { conn_type: ClientConnectionType::Server }`
//! - P2P disconnects: `NodeResult::PeerEvent { PeerSignal::Disconnect }` (handled in peer_event.rs)
//!
//! ## Design: SDK is Source of Truth
//! The SDK is the authoritative source for session state. When the SDK reports a disconnect,
//! we MUST clean up our internal state to mirror it. This ensures consistency between
//! the internal service layer and the underlying protocol layer.
//!
//! ## Distinction from Request Handler
//! - `requests/peer/disconnect.rs`: User-initiated (outbound) disconnect - calls SDK then cleans state
//! - `responses/disconnect.rs` (this file): SDK-initiated (inbound) C2S disconnect event

use crate::kernel::requests::peer::{cleanup_state, DisconnectedConnection};
use crate::kernel::{send_response_to_tcp_client, CitadelWorkspaceService};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{DisconnectNotification, InternalServiceResponse};
use citadel_sdk::prelude::{ClientConnectionType, Disconnect, NetworkError, Ratchet};

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

    // In SDK v0.13.1+, NodeResult::Disconnect only carries C2S connection types.
    // P2P disconnects are now handled via NodeResult::PeerEvent { PeerSignal::Disconnect }.
    if let Some(conn) = disconnect.conn_type {
        let cid = match conn {
            ClientConnectionType::Server { session_cid } => session_cid,
            ClientConnectionType::Extended { session_cid, .. } => session_cid,
        };

        citadel_sdk::logging::info!(
            target: "citadel",
            "[Disconnect Response] SDK reports C2S session {} disconnected - cleaning up internal state. Reason: {}",
            cid,
            disconnect.message
        );

        // SDK is source of truth - clean up session to mirror SDK state
        // NOTE: SDK has already disconnected, so we don't call disconnect_removed.
        // We just remove from our map and let the struct drop.
        if let Some(disconnected) = cleanup_state(&this.server_connection_map, cid, None) {
            let tcp_uuid = match &disconnected {
                DisconnectedConnection::C2S { tcp_uuid, .. } => *tcp_uuid,
                DisconnectedConnection::P2P { tcp_uuid, .. } => *tcp_uuid,
            };
            // Let the struct drop - SDK already disconnected so RAII is harmless
            drop(disconnected);

            let response =
                InternalServiceResponse::DisconnectNotification(DisconnectNotification {
                    cid,
                    peer_cid: None,
                    request_id: None,
                });
            return send_response_to_tcp_client(&this.tx_to_localhost_clients, response, tcp_uuid);
        }
    } else {
        citadel_sdk::logging::warn!(target: "citadel", "The disconnect request does not contain a connection type")
    }

    Ok(())
}

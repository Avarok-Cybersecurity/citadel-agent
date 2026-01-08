//! P2P (Peer-to-Peer) Event Handler
//!
//! This module handles SDK `NodeResult::PeerEvent` events, including:
//! - `PeerSignal::Disconnect`: P2P connection terminated
//! - `PeerSignal::PostRegister`: Peer registration request received
//! - `PeerSignal::PostConnect`: Peer connection request received
//! - `PeerSignal::BroadcastConnected`: Group broadcast event
//!
//! ## P2P Disconnect Flow (PeerSignal::Disconnect)
//! 1. Either peer calls `remote.find_target(cid, peer_cid).disconnect()`
//! 2. SDK sends `PeerSignal::Disconnect`, waits for `PeerEvent(PeerSignal::Disconnect)`
//! 3. This handler cleans up internal service state (removes peer from session)
//! 4. Notifies TCP client via `DisconnectNotification`
//!
//! ## Distinction from C2S Disconnect
//! - C2S: `NodeResult::Disconnect` - entire session terminated
//! - P2P: `PeerSignal::Disconnect` - single peer connection terminated
//!
//! Both use the shared `cleanup_state()` function for DRY state management.

use crate::kernel::requests::peer::cleanup_state;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    DisconnectNotification, InternalServiceResponse, PeerConnectNotification,
    PeerRegisterNotification,
};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{
    GroupEvent, NetworkError, PeerConnectionType, PeerEvent, PeerSignal, Ratchet,
};
use std::sync::atomic::Ordering;

/// Send response to TCP client with fallback to broadcast when target connection is stale.
/// This handles cases where a session's associated_tcp_connection has been closed
/// (e.g., in multi-tab browser scenarios with Playwright or reconnection scenarios).
async fn send_response_with_fallback<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    response: InternalServiceResponse,
    target_uuid: uuid::Uuid,
) -> Result<(), NetworkError> {
    let tcp_map = this.tx_to_localhost_clients.read();

    // First, try the target connection directly
    if let Some(sender) = tcp_map.get(&target_uuid) {
        return sender.send(response).map_err(|err| {
            NetworkError::Generic(format!("Failed to send response to TCP client: {err:?}"))
        });
    }

    // Target connection not found - broadcast to ALL active TCP connections
    // The clients will filter based on CID to only process messages meant for their sessions
    warn!(target: "citadel", "Target TCP connection {target_uuid:?} not found, broadcasting to all {} active connections", tcp_map.len());

    let mut sent_count = 0;
    for (uuid, sender) in tcp_map.iter() {
        if let Ok(()) = sender.send(response.clone()) {
            sent_count += 1;
            info!(target: "citadel", "Broadcast notification sent via TCP connection {:?}", uuid);
        }
    }

    if sent_count == 0 {
        warn!(target: "citadel", "No active TCP connections to broadcast to - notification will be lost");
    } else {
        info!(target: "citadel", "Successfully broadcast notification to {} TCP connections", sent_count);
    }

    Ok(())
}

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    event: PeerEvent,
) -> Result<(), NetworkError> {
    match event.event {
        PeerSignal::Disconnect {
            peer_conn_type:
                PeerConnectionType::LocalGroupPeer {
                    session_cid,
                    peer_cid,
                },
            disconnect_response: _,
        } => {
            // Use shared cleanup function (DRY)
            if let Some(conn_uuid) =
                cleanup_state(&this.server_connection_map, session_cid, Some(peer_cid))
            {
                let response =
                    InternalServiceResponse::DisconnectNotification(DisconnectNotification {
                        cid: session_cid,
                        peer_cid: Some(peer_cid),
                        request_id: None,
                    });
                // Use fallback function that broadcasts to all connections if target is stale
                send_response_with_fallback(this, response, conn_uuid).await?;
            }
        }
        PeerSignal::BroadcastConnected {
            session_cid,
            group_broadcast,
        } => {
            let evt = GroupEvent {
                session_cid,
                ticket: event.ticket,
                event: group_broadcast,
            };
            return super::group_event::handle(this, evt).await;
        }
        PeerSignal::PostRegister {
            peer_conn_type:
                PeerConnectionType::LocalGroupPeer {
                    session_cid: peer_cid,
                    peer_cid: session_cid,
                },
            inviter_username,
            invitee_username: _,
            ticket_opt: _,
            invitee_response: _,
        } => {
            info!(target: "citadel", "User {session_cid:?} received Register Request from {peer_cid:?}");

            // Cache the peer's username for later use in ListRegisteredPeers
            // The SDK's get_local_group_mutual_peers may not return usernames
            if !inviter_username.is_empty() {
                let mut cache = this.peer_username_cache.write();
                cache.insert((session_cid, peer_cid), inviter_username.clone());
                info!(target: "citadel", "Cached username '{}' for peer {} (session {})", inviter_username, peer_cid, session_cid);
            }

            // Extract what we need from the lock, then drop it before any await
            let tcp_conn = {
                let server_connection_map = this.server_connection_map.read();
                server_connection_map
                    .get(&session_cid)
                    .map(|conn| conn.associated_localhost_connection.load(Ordering::Relaxed))
            }; // Lock dropped here

            if let Some(associated_tcp_connection) = tcp_conn {
                let response =
                    InternalServiceResponse::PeerRegisterNotification(PeerRegisterNotification {
                        cid: session_cid,
                        peer_cid,
                        peer_username: inviter_username,
                        request_id: None,
                    });
                // Use fallback function that broadcasts to all connections if target is stale
                send_response_with_fallback(this, response, associated_tcp_connection).await?;
            }
        }
        PeerSignal::PostConnect {
            peer_conn_type:
                PeerConnectionType::LocalGroupPeer {
                    session_cid: peer_cid,
                    peer_cid: session_cid,
                },
            ticket_opt: _,
            invitee_response: _,
            session_security_settings,
            udp_mode,
            session_password: _,
        } => {
            info!(target: "citadel", "User {session_cid:?} received Connect Request from {peer_cid:?}");

            // Store the pending signal for later acceptance via PeerConnectAccept
            // We reconstruct the signal since the match consumes the fields
            let pending_signal = PeerSignal::PostConnect {
                peer_conn_type: PeerConnectionType::LocalGroupPeer {
                    // Note: The original signal has session_cid/peer_cid swapped from our perspective
                    session_cid: peer_cid,
                    peer_cid: session_cid,
                },
                ticket_opt: Some(event.ticket),
                invitee_response: None,
                session_security_settings: session_security_settings.clone(),
                udp_mode,
                session_password: None,
            };
            {
                let mut signals = this.pending_peer_connect_signals.write();
                signals.insert((session_cid, peer_cid), pending_signal);
                info!(target: "citadel", "[PostConnect] Stored pending PeerConnect signal for (cid={}, peer_cid={}), total pending: {}", session_cid, peer_cid, signals.len());
            }

            // Extract what we need from the lock, then drop it before any await
            let tcp_conn = {
                let server_connection_map = this.server_connection_map.read();
                server_connection_map
                    .get(&session_cid)
                    .map(|conn| conn.associated_localhost_connection.load(Ordering::Relaxed))
            }; // Lock dropped here

            if let Some(associated_tcp_connection) = tcp_conn {
                let response =
                    InternalServiceResponse::PeerConnectNotification(PeerConnectNotification {
                        cid: session_cid,
                        peer_cid,
                        session_security_settings,
                        udp_mode,
                        request_id: None,
                    });
                // Use fallback function that broadcasts to all connections if target is stale
                send_response_with_fallback(this, response, associated_tcp_connection).await?;
            }
        }
        _ => {}
    }

    Ok(())
}

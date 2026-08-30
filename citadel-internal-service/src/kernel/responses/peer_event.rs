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

use crate::kernel::requests::peer::{cleanup_state, DisconnectedConnection};
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

/// Deliver a peer notification to the session it belongs to.
///
/// The uuid carried on the event is the one recorded when the connection was
/// made, and it goes stale: a page reload, a tab close, a reconnect all mint a
/// new localhost connection while the session and its CID persist. That is real,
/// and it is why a fallback existed.
///
/// The fallback was to BROADCAST to every active localhost connection, on the
/// reasoning that "the clients will filter based on CID". Every browser on this
/// internal service — every account signed in on this machine — therefore
/// received the peer-register, peer-connect and disconnect notifications of
/// every other, and was trusted to discard them. Fail-open, on the client, for
/// data about who else is talking to whom.
///
/// The session already knows its current connection:
/// `associated_localhost_connection` is an `AtomicUuid` updated on reconnect. So
/// the stale uuid is re-resolved through the CID rather than abandoned, and when
/// even that finds nothing the notification is DROPPED with a warning — which is
/// exactly what `send_response_to_tcp_client` in kernel/mod.rs does, and what
/// this function was the only remaining exception to.
async fn send_response_for_session<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    response: InternalServiceResponse,
    session_cid: u64,
    recorded_uuid: uuid::Uuid,
) -> Result<(), NetworkError> {
    // The live uuid for this CID, if the session is still around. Read and
    // released before touching the client map, so the two locks are never held
    // together.
    let live_uuid: Option<uuid::Uuid> = {
        let connections = this.server_connection_map.read();
        connections.get(&session_cid).map(|connection| {
            connection
                .associated_localhost_connection
                .load(Ordering::Relaxed)
        })
    };

    let target = live_uuid.unwrap_or(recorded_uuid);
    if live_uuid.is_some_and(|live| live != recorded_uuid) {
        info!(target: "citadel", "Peer notification for CID {session_cid} re-resolved from stale {recorded_uuid:?} to {target:?}");
    }

    let tcp_map = this.tx_to_localhost_clients.read();
    match tcp_map.get(&target) {
        Some(sender) => sender.send(response).map_err(|err| {
            NetworkError::generic(format!("Failed to send response to TCP client: {err:?}"))
        }),
        None => {
            // Dropped, not broadcast. A notification nobody is listening for is
            // lost; a notification sent to everybody is a disclosure.
            warn!(target: "citadel", "No localhost connection for CID {session_cid} (tried {target:?}) - peer notification dropped");
            Ok(())
        }
    }
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
            ..
        } => {
            // SDK is source of truth - clean up P2P peer state to mirror SDK
            info!(
                target: "citadel",
                "[P2P Disconnect] SDK reports peer {} disconnected from session {} - cleaning up internal state",
                peer_cid,
                session_cid
            );

            // Use shared cleanup function (DRY)
            // NOTE: For SDK-initiated P2P disconnect events, the SDK has already disconnected.
            // We just remove from our map and let the struct drop (RAII is harmless).
            if let Some(disconnected) =
                cleanup_state(&this.server_connection_map, session_cid, Some(peer_cid))
            {
                let tcp_uuid = match &disconnected {
                    DisconnectedConnection::C2S { tcp_uuid, .. } => *tcp_uuid,
                    DisconnectedConnection::P2P { tcp_uuid, .. } => *tcp_uuid,
                };
                // Let the struct drop - SDK already disconnected so RAII is harmless
                drop(disconnected);

                let response =
                    InternalServiceResponse::DisconnectNotification(DisconnectNotification {
                        cid: session_cid,
                        peer_cid: Some(peer_cid),
                        request_id: None,
                    });
                // Re-resolved through the CID, never broadcast; see the function.
                send_response_for_session(this, response, session_cid, tcp_uuid).await?;
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

            // Store the pending signal for later acceptance via PeerRegisterRespond
            // The signal is stored with the original structure (CIDs as received from SDK)
            // The responses::peer_register() function will handle the reversal
            let pending_signal = PeerSignal::PostRegister {
                peer_conn_type: PeerConnectionType::LocalGroupPeer {
                    session_cid: peer_cid,
                    peer_cid: session_cid,
                },
                inviter_username: inviter_username.clone(),
                invitee_username: None,
                ticket_opt: Some(event.ticket),
                invitee_response: None,
            };
            {
                let mut signals = this.pending_peer_registrations.write();
                signals.insert((session_cid, peer_cid), pending_signal);
                info!(target: "citadel", "[PostRegister] Stored pending registration signal for (cid={}, peer_cid={}), total pending: {}", session_cid, peer_cid, signals.len());
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
                // Re-resolved through the CID, never broadcast; see the function.
                send_response_for_session(this, response, session_cid, associated_tcp_connection)
                    .await?;
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
                session_security_settings,
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
                // Re-resolved through the CID, never broadcast; see the function.
                send_response_for_session(this, response, session_cid, associated_tcp_connection)
                    .await?;
            }
        }
        _ => {}
    }

    Ok(())
}

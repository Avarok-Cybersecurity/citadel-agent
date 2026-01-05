use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::*;
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::*;
use std::sync::atomic::Ordering;
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    conn_id: Uuid,
    command: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    if let InternalServiceRequest::ConnectionManagement {
        request_id,
        management_command,
    } = command
    {
        info!(target: "citadel", "Handling connection management command: {:?}", management_command);

        let response = match management_command {
            ConfigCommand::SetConnectionOrphan {
                allow_orphan_sessions,
            } => {
                // Set orphan mode for this connection
                this.orphan_sessions
                    .write()
                    .insert(conn_id, allow_orphan_sessions);

                let message = if allow_orphan_sessions {
                    "Orphan mode enabled for connection"
                } else {
                    "Orphan mode disabled for connection"
                };

                InternalServiceResponse::ConnectionManagementSuccess(ConnectionManagementSuccess {
                    cid: 0, // Connection management is not associated with a specific session
                    request_id: Some(request_id),
                    message: message.to_string(),
                })
            }

            ConfigCommand::ClaimSession {
                session_cid,
                only_if_orphaned,
            } => {
                // Step 1: Check if session exists in internal service and get basic info
                let (old_conn_id, is_orphaned) = {
                    let server_connection_map = this.server_connection_map.read();
                    if let Some(connection) = server_connection_map.get(&session_cid) {
                        let old_conn_id = connection
                            .associated_localhost_connection
                            .load(Ordering::Relaxed);
                        let is_orphaned = !this
                            .tx_to_localhost_clients
                            .read()
                            .contains_key(&old_conn_id);
                        (old_conn_id, is_orphaned)
                    } else {
                        return Some(HandledRequestResult {
                            response: InternalServiceResponse::ConnectionManagementFailure(
                                ConnectionManagementFailure {
                                    cid: session_cid,
                                    request_id: Some(request_id),
                                    error: format!("Session {} not found", session_cid),
                                },
                            ),
                            uuid: conn_id,
                        });
                    }
                };

                // Step 2: Check orphan requirement
                if only_if_orphaned && !is_orphaned {
                    return Some(HandledRequestResult {
                        response: InternalServiceResponse::ConnectionManagementFailure(
                            ConnectionManagementFailure {
                                cid: session_cid,
                                request_id: Some(request_id),
                                error: format!("Session {} is not orphaned", session_cid),
                            },
                        ),
                        uuid: conn_id,
                    });
                }

                // Step 3: Verify session is active in SDK before allowing claim
                let remote = this.remote();
                let sdk_active_cids: Vec<u64> = match remote
                    .sessions()
                    .await
                {
                    Ok(conns) => {
                        conns.sessions.into_iter().map(|s| s.cid).collect()
                    }
                    Err(e) => {
                        warn!(target: "citadel", "ClaimSession: Failed to query SDK sessions: {:?}", e);
                        vec![]
                    }
                };

                info!(target: "citadel", "ClaimSession: SDK reports {} active sessions: {:?}", sdk_active_cids.len(), sdk_active_cids);

                // Step 4: Check if session is active in SDK
                if !sdk_active_cids.contains(&session_cid) {
                    // Session exists in internal service but not in SDK - clean up and deny
                    let mut server_connection_map = this.server_connection_map.write();
                    server_connection_map.remove(&session_cid);
                    info!(target: "citadel", "ClaimSession: Session {} removed - not active in SDK", session_cid);
                    return Some(HandledRequestResult {
                        response: InternalServiceResponse::ConnectionManagementFailure(
                            ConnectionManagementFailure {
                                cid: session_cid,
                                request_id: Some(request_id),
                                error: format!(
                                    "Session {} is not claimable: SDK session is disconnected",
                                    session_cid
                                ),
                            },
                        ),
                        uuid: conn_id,
                    });
                }

                // Step 5: Session is valid in both internal service and SDK - proceed with claim
                let mut server_connection_map = this.server_connection_map.write();

                // Find ALL sessions that share the same old TCP connection
                // This ensures all sessions from the same browser/client get updated together
                let sessions_to_update: Vec<u64> = server_connection_map
                    .iter()
                    .filter(|(_, conn)| {
                        conn.associated_localhost_connection.load(Ordering::Relaxed) == old_conn_id
                    })
                    .map(|(cid, _)| *cid)
                    .collect();

                let updated_count = sessions_to_update.len();

                // Update all sessions that shared the old TCP connection to use the new one
                // NOTE: We do NOT clear peer connections - the SDK P2P connections are still
                // active even though the TCP connection to internal service was dropped.
                // The AsyncSink channels in PeerConnection are SDK-layer, not TCP-layer.
                for cid in &sessions_to_update {
                    if let Some(conn) = server_connection_map.get_mut(cid) {
                        conn.associated_localhost_connection
                            .store(conn_id, Ordering::Relaxed);
                        let peer_count = conn.peers.len();
                        if peer_count > 0 {
                            info!(target: "citadel", "ClaimSession: Session {} has {} existing peer connections (preserved)", cid, peer_count);
                        }
                    }
                }

                info!(target: "citadel", "ClaimSession: Updated {} sessions from old TCP connection {:?} to new {:?}", updated_count, old_conn_id, conn_id);

                // Add this connection to orphan mode to preserve it when the new connection drops
                this.orphan_sessions.write().insert(conn_id, true);

                InternalServiceResponse::ConnectionManagementSuccess(ConnectionManagementSuccess {
                    cid: session_cid,
                    request_id: Some(request_id),
                    message: format!(
                        "Successfully claimed session {} (updated {} related sessions)",
                        session_cid, updated_count
                    ),
                })
            }

            ConfigCommand::DisconnectOrphan { session_cid } => {
                let mut server_connection_map = this.server_connection_map.write();

                if let Some(session_cid) = session_cid {
                    // Disconnect specific orphan session
                    if let Some(_connection) = server_connection_map.remove(&session_cid) {
                        InternalServiceResponse::ConnectionManagementSuccess(
                            ConnectionManagementSuccess {
                                cid: session_cid,
                                request_id: Some(request_id),
                                message: format!("Disconnected orphan session {}", session_cid),
                            },
                        )
                    } else {
                        InternalServiceResponse::ConnectionManagementFailure(
                            ConnectionManagementFailure {
                                cid: session_cid,
                                request_id: Some(request_id),
                                error: format!("Orphan session {} not found", session_cid),
                            },
                        )
                    }
                } else {
                    // Disconnect all orphan sessions
                    let tcp_connection_map = this.tx_to_localhost_clients.read();
                    let orphaned_sessions: Vec<u64> = server_connection_map
                        .iter()
                        .filter(|(_, connection)| {
                            let conn_id = connection
                                .associated_localhost_connection
                                .load(Ordering::Relaxed);
                            !tcp_connection_map.contains_key(&conn_id)
                        })
                        .map(|(cid, _)| *cid)
                        .collect();

                    drop(tcp_connection_map);

                    let count = orphaned_sessions.len();
                    for cid in orphaned_sessions {
                        server_connection_map.remove(&cid);
                    }

                    InternalServiceResponse::ConnectionManagementSuccess(
                        ConnectionManagementSuccess {
                            cid: 0, // No specific session for bulk disconnect
                            request_id: Some(request_id),
                            message: format!("Disconnected {} orphan sessions", count),
                        },
                    )
                }
            }

            ConfigCommand::ReleaseSession { session_cid } => {
                // Mark the session as "released" - simulate orphan by setting associated_tcp_connection
                // to a UUID that's not in tcp_connection_map, making it appear orphaned.
                // The session stays in server_connection_map and becomes immediately claimable.

                let server_connection_map = this.server_connection_map.read();
                if let Some(connection) = server_connection_map.get(&session_cid) {
                    // Use nil UUID to mark as orphaned - this UUID won't exist in tcp_connection_map
                    let orphan_marker = Uuid::nil();
                    connection
                        .associated_localhost_connection
                        .store(orphan_marker, Ordering::Relaxed);

                    info!(target: "citadel", "ReleaseSession: Session {} marked as orphaned (released by tab)", session_cid);

                    InternalServiceResponse::ConnectionManagementSuccess(
                        ConnectionManagementSuccess {
                            cid: session_cid,
                            request_id: Some(request_id),
                            message: format!(
                                "Session {} released and marked as orphaned",
                                session_cid
                            ),
                        },
                    )
                } else {
                    InternalServiceResponse::ConnectionManagementFailure(
                        ConnectionManagementFailure {
                            cid: session_cid,
                            request_id: Some(request_id),
                            error: format!("Session {} not found", session_cid),
                        },
                    )
                }
            }
        };

        Some(HandledRequestResult {
            response,
            uuid: conn_id,
        })
    } else {
        warn!(target: "citadel", "Connection management handler received wrong command type");
        None
    }
}

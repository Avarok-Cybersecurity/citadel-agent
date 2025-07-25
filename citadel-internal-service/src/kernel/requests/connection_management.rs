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
            ConfigCommand::SetConnectionOrphan { allow_orphan_sessions } => {
                // Set orphan mode for this connection
                this.orphan_sessions.lock().await.insert(conn_id, allow_orphan_sessions);
                
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
            
            ConfigCommand::ClaimSession { session_cid, only_if_orphaned } => {
                let mut server_connection_map = this.server_connection_map.lock().await;
                
                if let Some(connection) = server_connection_map.get_mut(&session_cid) {
                    let current_conn_id = connection.associated_tcp_connection.load(Ordering::Relaxed);
                    
                    // Check if the session is orphaned (not associated with any active TCP connection)
                    let is_orphaned = !this.tcp_connection_map.lock().await.contains_key(&current_conn_id);
                    
                    if !only_if_orphaned || is_orphaned {
                        // Update the associated TCP connection to the current one
                        connection.associated_tcp_connection.store(conn_id, Ordering::Relaxed);
                        
                        // Add this connection to orphan mode to preserve it when the new connection drops
                        this.orphan_sessions.lock().await.insert(conn_id, true);
                        
                        InternalServiceResponse::ConnectionManagementSuccess(ConnectionManagementSuccess {
                            cid: session_cid,
                            request_id: Some(request_id),
                            message: format!("Successfully claimed session {}", session_cid),
                        })
                    } else {
                        InternalServiceResponse::ConnectionManagementFailure(ConnectionManagementFailure {
                            cid: session_cid,
                            request_id: Some(request_id),
                            error: format!("Session {} is not orphaned", session_cid),
                        })
                    }
                } else {
                    InternalServiceResponse::ConnectionManagementFailure(ConnectionManagementFailure {
                        cid: session_cid,
                        request_id: Some(request_id),
                        error: format!("Session {} not found", session_cid),
                    })
                }
            }
            
            ConfigCommand::DisconnectOrphan { session_cid } => {
                let mut server_connection_map = this.server_connection_map.lock().await;
                
                if let Some(session_cid) = session_cid {
                    // Disconnect specific orphan session
                    if let Some(_connection) = server_connection_map.remove(&session_cid) {
                        InternalServiceResponse::ConnectionManagementSuccess(ConnectionManagementSuccess {
                            cid: session_cid,
                            request_id: Some(request_id),
                            message: format!("Disconnected orphan session {}", session_cid),
                        })
                    } else {
                        InternalServiceResponse::ConnectionManagementFailure(ConnectionManagementFailure {
                            cid: session_cid,
                            request_id: Some(request_id),
                            error: format!("Orphan session {} not found", session_cid),
                        })
                    }
                } else {
                    // Disconnect all orphan sessions
                    let tcp_connection_map = this.tcp_connection_map.lock().await;
                    let orphaned_sessions: Vec<u64> = server_connection_map
                        .iter()
                        .filter(|(_, connection)| {
                            let conn_id = connection.associated_tcp_connection.load(Ordering::Relaxed);
                            !tcp_connection_map.contains_key(&conn_id)
                        })
                        .map(|(cid, _)| *cid)
                        .collect();
                    
                    drop(tcp_connection_map);
                    
                    let count = orphaned_sessions.len();
                    for cid in orphaned_sessions {
                        server_connection_map.remove(&cid);
                    }
                    
                    InternalServiceResponse::ConnectionManagementSuccess(ConnectionManagementSuccess {
                        cid: 0, // No specific session for bulk disconnect
                        request_id: Some(request_id),
                        message: format!("Disconnected {} orphan sessions", count),
                    })
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
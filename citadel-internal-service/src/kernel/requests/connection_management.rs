use crate::kernel::requests::connection_management_auth::{
    may_disconnect, may_release, Authorization, SessionOwner,
};
use crate::kernel::requests::connection_management_claim;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::*;
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::*;
use std::sync::atomic::Ordering;
use uuid::Uuid;

/// What the connection map says about `session_cid`, or `None` if there is no
/// such session.
///
/// "Orphaned" is decided the same way every other site in this file decides it:
/// the owning uuid is no longer in `tx_to_localhost_clients`. Read here, once,
/// so the authorization decision and the `only_if_orphaned` check cannot drift
/// apart into disagreeing about the same session.
pub(super) fn owner_of<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    session_cid: u64,
) -> Option<SessionOwner> {
    let owner_uuid = {
        let map = this.server_connection_map.read();
        map.get(&session_cid)
            .map(|conn| conn.associated_localhost_connection.load(Ordering::Relaxed))?
    };
    let live = this
        .tx_to_localhost_clients
        .read()
        .contains_key(&owner_uuid);
    Some(if live {
        SessionOwner::Live(owner_uuid)
    } else {
        SessionOwner::Orphaned
    })
}

/// Turn a refusal into the response the caller gets.
pub(super) fn refusal(
    session_cid: u64,
    request_id: Uuid,
    conn_id: Uuid,
    error: String,
) -> HandledRequestResult {
    warn!(target: "citadel", "ConnectionManagement REFUSED for session {session_cid} from connection {conn_id}: {error}");
    HandledRequestResult {
        response: InternalServiceResponse::ConnectionManagementFailure(
            ConnectionManagementFailure {
                cid: session_cid,
                request_id: Some(request_id),
                error,
            },
        ),
        uuid: conn_id,
    }
}

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
                return connection_management_claim::claim_session(
                    this,
                    conn_id,
                    request_id,
                    session_cid,
                    only_if_orphaned,
                )
                .await
            }

            ConfigCommand::DisconnectOrphan { session_cid } => {
                let mut server_connection_map = this.server_connection_map.write();

                if let Some(session_cid) = session_cid {
                    // "Orphan" was never checked here: this removed any session
                    // the caller could name, live or not, from any connection.
                    let owner = {
                        let owner_uuid = server_connection_map.get(&session_cid).map(|conn| {
                            conn.associated_localhost_connection.load(Ordering::Relaxed)
                        });
                        owner_uuid.map(|uuid| {
                            if this.tx_to_localhost_clients.read().contains_key(&uuid) {
                                SessionOwner::Live(uuid)
                            } else {
                                SessionOwner::Orphaned
                            }
                        })
                    };
                    if let Some(owner) = owner {
                        if let Authorization::Refuse(error) =
                            may_disconnect(owner, conn_id, session_cid)
                        {
                            drop(server_connection_map);
                            return Some(refusal(session_cid, request_id, conn_id, error));
                        }
                    }

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

                // Releasing means "this tab is done with it". Releasing a
                // session another connection is actively using marked it
                // reclaimable out from under its owner.
                if let Some(owner) = owner_of(this, session_cid) {
                    if let Authorization::Refuse(error) = may_release(owner, conn_id, session_cid) {
                        return Some(refusal(session_cid, request_id, conn_id, error));
                    }
                }

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

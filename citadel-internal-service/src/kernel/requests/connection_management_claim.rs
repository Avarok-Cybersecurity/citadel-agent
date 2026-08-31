//! Claiming a session for this localhost connection.
//!
//! Split out of `connection_management.rs`: this arm is longer than the other
//! three put together, and the file was over the 250-line cap. The
//! authorization rule it enforces lives in `connection_management_auth.rs`,
//! shared with the other two session-mutating commands so the three cannot
//! drift apart.

use crate::kernel::requests::connection_management::{owner_of, refusal};
use crate::kernel::requests::connection_management_auth::{may_claim, Authorization};
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::*;
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::*;
use std::sync::atomic::Ordering;
use uuid::Uuid;

pub(super) async fn claim_session<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    conn_id: Uuid,
    request_id: Uuid,
    session_cid: u64,
    only_if_orphaned: bool,
) -> Option<HandledRequestResult> {
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

    // Step 2b: A live session held by a DIFFERENT localhost
    // connection is not claimable, whatever `only_if_orphaned`
    // says. That flag is supplied by the caller, so it authorized
    // nothing: `false` meant "take it from whoever has it".
    // See connection_management_auth.rs.
    if let Some(owner) = owner_of(this, session_cid) {
        if let Authorization::Refuse(error) = may_claim(owner, conn_id, session_cid) {
            return Some(refusal(session_cid, request_id, conn_id, error));
        }
    }

    // Step 3: Verify session is active in SDK before allowing claim
    let remote = this.remote();
    let sdk_active_cids: Vec<u64> = match remote.sessions().await {
        Ok(conns) => conns.sessions.into_iter().map(|s| s.cid).collect(),
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

    Some(HandledRequestResult {
        response: InternalServiceResponse::ConnectionManagementSuccess(
            ConnectionManagementSuccess {
                cid: session_cid,
                request_id: Some(request_id),
                message: format!(
                    "Successfully claimed session {} (updated {} related sessions)",
                    session_cid, updated_count
                ),
            },
        ),
        uuid: conn_id,
    })
}

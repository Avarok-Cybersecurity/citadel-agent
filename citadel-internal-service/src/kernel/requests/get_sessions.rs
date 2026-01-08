use crate::kernel::requests::peer::cleanup_state;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    GetSessionsResponse, InternalServiceRequest, InternalServiceResponse, PeerSessionInformation,
    SessionInformation,
};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{ProtocolRemoteExt, Ratchet, TargetLockedRemote};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::GetSessions { request_id } = request else {
        unreachable!("Should never happen if programmed properly")
    };

    // Log current state at request start
    {
        let lock = this.server_connection_map.read();
        let session_cids: Vec<u64> = lock.keys().copied().collect();
        let session_usernames: Vec<String> = lock.values().map(|c| c.username.clone()).collect();
        info!(target: "citadel", "GetSessions: Request received. server_connection_map has {} sessions. CIDs: {:?}, Usernames: {:?}",
            lock.len(), session_cids, session_usernames);
    }

    // Step 1: Query SDK for authoritative C2S session state
    // NOTE: We only reconcile C2S sessions, not P2P connections. The SDK's sessions()
    // returns active C2S sessions, but P2P connections in the internal service are
    // tracked separately and may persist across reconnections (orphan mode).
    let sdk_c2s_sessions: HashSet<u64> = match this.remote().sessions().await {
        Ok(sdk_sessions) => {
            let sessions: HashSet<u64> = sdk_sessions.sessions.iter().map(|s| s.cid).collect();
            info!(target: "citadel", "GetSessions: SDK reports {} C2S sessions", sessions.len());
            sessions
        }
        Err(e) => {
            warn!(target: "citadel", "GetSessions: Failed to query SDK sessions: {:?}. Skipping reconciliation.", e);
            // Fallback to returning internal state without reconciliation
            return Some(build_response_from_internal_state(this, uuid, request_id));
        }
    };

    // Step 2: Find stale C2S sessions (not P2P connections!)
    let stale_c2s_sessions: Vec<u64> = {
        let lock = this.server_connection_map.read();
        lock.keys()
            .filter(|cid| !sdk_c2s_sessions.contains(cid))
            .copied()
            .collect()
    };

    // Step 3: Clean up stale C2S sessions only
    if !stale_c2s_sessions.is_empty() {
        info!(target: "citadel", "GetSessions: Cleaning up {} stale C2S sessions", stale_c2s_sessions.len());
        for cid in stale_c2s_sessions {
            info!(target: "citadel", "GetSessions: Removing stale C2S session {}", cid);
            cleanup_state(
                &this.server_connection_map,
                cid,
                None, // Only clean C2S session, peers will be removed with it
            );
        }
    }

    // Step 4: Build and return response from reconciled state
    Some(build_response_from_internal_state(this, uuid, request_id))
}

/// Helper function to build GetSessionsResponse from current internal state
fn build_response_from_internal_state<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request_id: Uuid,
) -> HandledRequestResult {
    let lock = this.server_connection_map.read();
    let username_cache = this.peer_username_cache.read();
    let mut sessions = Vec::new();

    info!(target: "citadel", "GetSessions: Found {} total sessions in server_connection_map", lock.len());

    for (cid, connection) in lock.iter() {
        let conn_id = connection
            .associated_localhost_connection
            .load(Ordering::Relaxed);
        info!(target: "citadel", "GetSessions: Session {} for user {} associated with connection {}", cid, connection.username, conn_id);

        let mut session = SessionInformation {
            cid: *cid,
            username: connection.username.clone(),
            server_address: connection.server_address.clone(),
            peer_connections: HashMap::new(),
        };

        for (peer_cid, conn) in connection.peers.iter() {
            // Try remote username first, then fall back to cached username
            let peer_username = conn
                .remote
                .as_ref()
                .and_then(|r| r.target_username())
                .map(ToString::to_string)
                .or_else(|| username_cache.get(&(*cid, *peer_cid)).cloned())
                .unwrap_or_default();

            session.peer_connections.insert(
                *peer_cid,
                PeerSessionInformation {
                    cid: *cid,
                    peer_cid: *peer_cid,
                    peer_username,
                },
            );
        }
        sessions.push(session);
    }

    let response = InternalServiceResponse::GetSessionsResponse(GetSessionsResponse {
        cid: 0,
        sessions,
        request_id: Some(request_id),
    });

    HandledRequestResult { response, uuid }
}

use crate::kernel::requests::HandledRequestResult;
use crate::kernel::{CitadelWorkspaceService, Connection};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    DisconnectNotification, InternalServiceRequest, InternalServiceResponse, PeerDisconnectFailure,
};
use citadel_sdk::prelude::{
    ClientServerRemote, NetworkError, NodeRemote, ProtocolRemoteExt, ProtocolRemoteTargetExt,
    Ratchet, VirtualTargetType,
};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::time::timeout;

use uuid::Uuid;

/// Timeout for SDK disconnect operations.
/// User-initiated disconnects should not hang indefinitely - if the SDK doesn't
/// respond within this time, we proceed with internal state cleanup anyway.
const SDK_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Disconnects a peer or C2S connection at the SDK/protocol layer.
///
/// On success, guarantees the peer/session has been disconnected from the network AND cleared from the SDK.
/// This function does NOT clear the session from the InternalService's server_connection_map.
/// The caller should call `cleanup_state` after this succeeds.
///
/// # Arguments
/// * `remote` - NodeRemote for SDK operations
/// * `request_id` - UUID for tracking the request
/// * `cid` - Session CID
/// * `peer_cid` - Some(peer_cid) for P2P disconnect, None for C2S disconnect
pub async fn disconnect_any<R: Ratchet>(
    remote: &NodeRemote<R>,
    request_id: Uuid,
    cid: u64,
    peer_cid: Option<u64>,
) -> Result<InternalServiceResponse, NetworkError> {
    if let Some(target_cid) = peer_cid {
        remote
            .find_target(cid, target_cid)
            .await?
            .disconnect()
            .await?;
    } else {
        // Construct a c2s remote purely for the purpose of disconnecting from the central server
        ClientServerRemote::new(
            VirtualTargetType::LocalGroupServer { session_cid: cid },
            remote.clone(),
            Default::default(),
            None,
            None,
        )
        .disconnect()
        .await?;
    }

    // Perform a sanity check: verify session/peer is no longer in SDK
    let still_in_protocol = remote.sessions().await.map(|sessions| {
        let conn = sessions.sessions.iter().find(|sess| sess.cid == cid);
        if let Some(conn) = conn {
            if let Some(peer_cid) = peer_cid {
                conn.connections
                    .iter()
                    .any(|c| c.peer_cid.unwrap_or(0) == peer_cid)
            } else {
                // C2S still exists
                true
            }
        } else {
            false
        }
    })?;

    if still_in_protocol {
        return Err(NetworkError::msg(format!(
            "Client or peer is still in protocol: cid={cid}, peer_cid={peer_cid:?}"
        )));
    }

    Ok(InternalServiceResponse::DisconnectNotification(
        DisconnectNotification {
            cid,
            peer_cid,
            request_id: Some(request_id),
        },
    ))
}

/// Clears internal service state for a session or peer connection.
/// SINGLE SOURCE OF TRUTH for state cleanup (DRY).
///
/// Called by:
/// - Request handlers (after SDK disconnect)
/// - Response handlers (on SDK disconnect events)
///
/// This function only clears internal service state - it does NOT disconnect at the SDK layer.
/// For request handlers, call `disconnect_any` first to ensure the SDK has cleaned up.
/// For response handlers, the SDK has already disconnected, so just clean up state.
///
/// # Arguments
/// * `server_connection_map` - The internal service's session map
/// * `cid` - Session CID
/// * `peer_cid` - Some(peer_cid) for P2P cleanup, None for C2S cleanup
///
/// # Returns
/// The UUID of the TCP connection that was associated with this session/peer,
/// or None if the session/peer was already removed.
pub fn cleanup_state<R: Ratchet>(
    server_connection_map: &Arc<RwLock<HashMap<u64, Connection<R>>>>,
    cid: u64,
    peer_cid: Option<u64>,
) -> Option<Uuid> {
    use std::sync::atomic::Ordering;

    if let Some(target_cid) = peer_cid {
        // P2P cleanup
        let mut lock = server_connection_map.write();
        if let Some(sess) = lock.get_mut(&cid) {
            if let Some(peer_sess) = sess.peers.remove(&target_cid) {
                citadel_sdk::logging::info!(
                    "[cleanup_state] Removed peer {target_cid} from session {cid}"
                );
                return Some(peer_sess.associated_localhost_connection.load(Ordering::Relaxed));
            }
        }
        citadel_sdk::logging::warn!(
            "[cleanup_state] Peer {target_cid} already removed from session {cid}"
        );
        None
    } else {
        // C2S cleanup
        let mut lock = server_connection_map.write();
        let count_before = lock.len();
        let session_keys: Vec<u64> = lock.keys().copied().collect();
        citadel_sdk::logging::info!(
            "[cleanup_state] BEFORE removal: {} sessions in map, CIDs: {:?}",
            count_before,
            session_keys
        );
        if let Some(sess) = lock.remove(&cid) {
            let count_after = lock.len();
            let remaining_keys: Vec<u64> = lock.keys().copied().collect();
            citadel_sdk::logging::info!(
                "[cleanup_state] Removed C2S session {cid}. AFTER removal: {} sessions, remaining CIDs: {:?}",
                count_after,
                remaining_keys
            );
            return Some(sess.associated_localhost_connection.load(Ordering::Relaxed));
        }
        citadel_sdk::logging::warn!(
            "[cleanup_state] Session {cid} already removed (not in map)"
        );
        None
    }
}

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let (request_id, cid, peer_cid) = match request {
        InternalServiceRequest::PeerDisconnect {
            request_id,
            cid,
            peer_cid,
        } => (request_id, cid, Some(peer_cid)),
        InternalServiceRequest::Disconnect { request_id, cid } => (request_id, cid, None),
        _ => unreachable!("Should never happen if programmed properly"),
    };

    // Check if connection exists in internal service state
    let (session_exists, peer_session_exists) = {
        let conns = this.server_connection_map.read();
        let session_exists = conns.contains_key(&cid);
        let peer_session_exists = session_exists
            && peer_cid
                .map(|p| {
                    conns
                        .get(&cid)
                        .map(|conn| conn.peers.contains_key(&p))
                        .unwrap_or(false)
                })
                .unwrap_or(false);
        (session_exists, peer_session_exists)
    };

    if !session_exists {
        return Some(HandledRequestResult {
            response: InternalServiceResponse::PeerDisconnectFailure(PeerDisconnectFailure {
                cid,
                message: "disconnect: Server connection not found".to_string(),
                request_id: Some(request_id),
            }),
            uuid,
        });
    }

    if peer_cid.is_some() && !peer_session_exists {
        return Some(HandledRequestResult {
            response: InternalServiceResponse::PeerDisconnectFailure(PeerDisconnectFailure {
                cid,
                message: "disconnect: Peer connection not found".to_string(),
                request_id: Some(request_id),
            }),
            uuid,
        });
    }

    // CRITICAL FIX: Clear orphan mode BEFORE disconnecting at SDK layer
    // This prevents a race condition where the WebSocket drops during/after disconnect,
    // causing the orphan handler to preserve the session that we're trying to remove.
    // By clearing orphan mode first, the orphan handler in ext.rs will remove (not preserve)
    // the session if the WebSocket drops before cleanup_state() runs.
    if peer_cid.is_none() {
        // Only for C2S disconnects (full session disconnect)
        let conn_id = {
            let conns = this.server_connection_map.read();
            conns
                .get(&cid)
                .map(|conn| conn.associated_localhost_connection.load(Ordering::Relaxed))
        };
        if let Some(conn_id) = conn_id {
            citadel_sdk::logging::info!(
                "[Disconnect] Clearing orphan mode for connection {:?} before disconnecting CID {}",
                conn_id,
                cid
            );
            this.orphan_sessions.write().remove(&conn_id);
        }
    }

    // Stage 1: Disconnect at the protocol/SDK layer with timeout
    // The user explicitly requested disconnect - we honor that intent even if SDK has issues.
    // After timeout or error, we STILL proceed with internal state cleanup to avoid orphaned sessions.
    let sdk_disconnect_result =
        timeout(SDK_DISCONNECT_TIMEOUT, disconnect_any(this.remote(), request_id, cid, peer_cid)).await;

    let sdk_warning = match sdk_disconnect_result {
        Ok(Ok(_)) => {
            citadel_sdk::logging::info!(
                "[Disconnect] SDK disconnect succeeded for CID {} peer_cid {:?}",
                cid,
                peer_cid
            );
            None
        }
        Ok(Err(err)) => {
            citadel_sdk::logging::warn!(
                "[Disconnect] SDK disconnect failed for CID {} peer_cid {:?}: {:?}. Proceeding with state cleanup.",
                cid,
                peer_cid,
                err
            );
            Some(format!("SDK disconnect failed: {err:?}"))
        }
        Err(_elapsed) => {
            citadel_sdk::logging::warn!(
                "[Disconnect] SDK disconnect timed out after {:?} for CID {} peer_cid {:?}. Proceeding with state cleanup.",
                SDK_DISCONNECT_TIMEOUT,
                cid,
                peer_cid
            );
            Some(format!("SDK disconnect timed out after {:?}", SDK_DISCONNECT_TIMEOUT))
        }
    };

    // Stage 2: ALWAYS clear the internal service state
    // This ensures the session is removed even if SDK disconnect failed/timed out.
    // The user explicitly requested disconnect - we must honor that intent.
    cleanup_state(&this.server_connection_map, cid, peer_cid);

    // Log warning if SDK disconnect had issues
    if let Some(warning) = sdk_warning {
        citadel_sdk::logging::info!(
            "[Disconnect] Completed with warning for CID {}: {}",
            cid,
            warning
        );
    }

    // Return success notification - the session is disconnected from internal service's perspective
    Some(HandledRequestResult {
        response: InternalServiceResponse::DisconnectNotification(DisconnectNotification {
            cid,
            peer_cid,
            request_id: Some(request_id),
        }),
        uuid,
    })
}

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
use uuid::Uuid;

/// Disconnects a peer or C2S connection at the SDK/protocol layer.
///
/// On success, guarantees the peer/session has been disconnected from the network AND cleared from the SDK.
/// This function does NOT clear the session from the InternalService's server_connection_map.
/// The caller should call `disconnect_for_internal_service_state` after this succeeds.
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

/// Clears the state for a P2P or C2S connection in the internal service's server_connection_map.
///
/// This function only clears internal service state - it does NOT disconnect at the SDK layer.
/// Call `disconnect_any` first to ensure the SDK has cleaned up.
///
/// # Arguments
/// * `server_connection_map` - The internal service's session map
/// * `request_id` - UUID for tracking the request
/// * `cid` - Session CID
/// * `peer_cid` - Some(peer_cid) for P2P state cleanup, None for C2S state cleanup
pub fn disconnect_for_internal_service_state<R: Ratchet>(
    server_connection_map: &Arc<RwLock<HashMap<u64, Connection<R>>>>,
    request_id: Uuid,
    cid: u64,
    peer_cid: Option<u64>,
) -> InternalServiceResponse {
    if let Some(target_cid) = peer_cid {
        let mut lock = server_connection_map.write();
        if let Some(sess) = lock.get_mut(&cid) {
            if let Some(_peer_sess) = sess.peers.remove(&target_cid) {
                citadel_sdk::logging::info!(
                    "disconnect: clear_state: Disconnected from peer session {target_cid}"
                );
            } else {
                // State cleared before this function had the chance to. Warning only, return success
                citadel_sdk::logging::warn!(
                    "disconnect: clear_state: Peer {target_cid} already removed from session {cid}"
                );
            }
        } else {
            // State cleared before this function had the chance to. Warning only, return success
            citadel_sdk::logging::warn!(
                "disconnect: clear_state: Session {cid} already removed before peer cleanup"
            );
        }
    } else {
        let mut lock = server_connection_map.write();
        if let Some(_sess) = lock.remove(&cid) {
            citadel_sdk::logging::info!(
                "disconnect: clear_state: Disconnected from c2s session {cid}"
            );
        } else {
            // State cleared before this function had the chance to. Warning only, return success
            citadel_sdk::logging::warn!(
                "disconnect: clear_state: Session {cid} already removed"
            );
        }
    }

    InternalServiceResponse::DisconnectNotification(DisconnectNotification {
        cid,
        peer_cid,
        request_id: Some(request_id),
    })
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

    // Stage 1: Disconnect at the protocol/SDK layer
    // On error, return immediately - do NOT proceed to state cleanup
    if let Err(err) = disconnect_any(this.remote(), request_id, cid, peer_cid).await {
        return Some(HandledRequestResult {
            response: InternalServiceResponse::PeerDisconnectFailure(PeerDisconnectFailure {
                cid,
                message: format!("disconnect: Failed to disconnect at SDK layer: {err:?}"),
                request_id: Some(request_id),
            }),
            uuid,
        });
    }

    // Stage 2: Clear the state in the internal service
    let response =
        disconnect_for_internal_service_state(&this.server_connection_map, request_id, cid, peer_cid);

    Some(HandledRequestResult { response, uuid })
}

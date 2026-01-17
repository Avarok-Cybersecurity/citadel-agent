use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, MessageNotification, PeerConnectFailure,
    PeerConnectSuccess,
};
use citadel_sdk::logging::{error, info, warn};
use citadel_sdk::prefabs::ClientServerRemote;
use citadel_sdk::prelude::{
    ProtocolRemoteExt, ProtocolRemoteTargetExt, Ratchet, VirtualTargetType,
};
use futures::StreamExt;
use uuid::Uuid;

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::PeerConnect {
        request_id,
        cid,
        peer_cid,
        udp_mode,
        session_security_settings,
        peer_session_password,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };

    info!(target: "citadel", "[PeerConnect] *** RECEIVED PeerConnect REQUEST *** cid={}, peer_cid={}, request_id={:?}", cid, peer_cid, request_id);

    let remote = this.remote();
    info!(target: "citadel", "[PeerConnect] Got remote, checking boundary conditions...");

    // Boundary check: sync internal state with SDK before connecting
    let peer_exists_in_internal = {
        let lock = this.server_connection_map.read();
        lock.get(&cid)
            .map(|conn| conn.peers.contains_key(&peer_cid))
            .unwrap_or(false)
    };

    if peer_exists_in_internal {
        info!(target: "citadel", "[PeerConnect] Peer {} exists in internal state, checking SDK...", peer_cid);

        // Query SDK to see if P2P connection actually exists
        let sdk_has_peer = match remote.sessions().await {
            Ok(sessions) => sessions
                .sessions
                .iter()
                .find(|s| s.cid == cid)
                .map(|s| s.connections.iter().any(|c| c.peer_cid == Some(peer_cid)))
                .unwrap_or(false),
            Err(e) => {
                info!(target: "citadel", "[PeerConnect] Failed to query SDK sessions: {:?}, assuming no peer", e);
                false
            }
        };

        if sdk_has_peer {
            // Both internal and SDK have peer → Hard error
            info!(target: "citadel", "[PeerConnect] BOUNDARY: Already connected to peer {} (both internal and SDK have it)", peer_cid);
            return Some(HandledRequestResult {
                response: InternalServiceResponse::PeerConnectFailure(PeerConnectFailure {
                    cid,
                    message: format!("Already connected to peer {}", peer_cid),
                    request_id: Some(request_id),
                }),
                uuid,
            });
        } else {
            // Internal has peer but SDK doesn't → Clear stale state
            info!(target: "citadel", "[PeerConnect] BOUNDARY: Clearing stale peer {} from session {} (SDK ratchet cleared)", peer_cid, cid);
            let mut lock = this.server_connection_map.write();
            if let Some(conn) = lock.get_mut(&cid) {
                conn.peers.remove(&peer_cid);
            }
            // Now proceed with fresh PeerConnect
        }
    }

    info!(target: "citadel", "[PeerConnect] Creating fresh ClientServerRemote for peer {}...", peer_cid);

    let client_to_server_remote = ClientServerRemote::new(
        VirtualTargetType::LocalGroupPeer {
            session_cid: cid,
            peer_cid,
        },
        remote.clone(),
        session_security_settings,
        None,
        None,
    );

    info!(target: "citadel", "[PeerConnect] Calling find_target({}, {})...", cid, peer_cid);
    let response = match client_to_server_remote.find_target(cid, peer_cid).await {
        Ok(symmetric_identifier_handle_ref) => {
            info!(target: "citadel", "[PeerConnect] find_target succeeded, calling connect_to_peer_custom with 30s timeout...");

            // Add timeout to prevent indefinite hanging
            let connect_future = symmetric_identifier_handle_ref.connect_to_peer_custom(
                session_security_settings,
                udp_mode,
                peer_session_password,
            );

            match tokio::time::timeout(std::time::Duration::from_secs(30), connect_future).await {
                Ok(connect_result) => match connect_result {
                    Ok(peer_connect_success) => {
                        info!(target: "citadel", "[PeerConnect] connect_to_peer_custom succeeded!");
                        let (sink, mut stream) = peer_connect_success.channel.split();
                        {
                            let mut map = this.server_connection_map.write();
                            if let Some(conn) = map.get_mut(&cid) {
                                conn.add_peer_connection(
                                    peer_cid,
                                    sink,
                                    peer_connect_success.remote,
                                );
                                info!(target: "citadel", "[PeerConnect] Added peer {} to cid {}'s peers. Total peers: {}", peer_cid, cid, conn.peers.len());
                            } else {
                                error!(target: "citadel", "[PeerConnect] CRITICAL: Cannot find session {} in server_connection_map to add peer {}", cid, peer_cid);
                            }
                        }

                        let hm_for_conn = this.tx_to_localhost_clients.clone();
                        let server_conn_map = this.server_connection_map.clone();

                        let connection_read_stream = async move {
                            info!(target:"citadel","[P2P-RECV-CONNECT] *** Starting P2P read stream for LOCAL_CID={cid} from PEER={peer_cid} ***");
                            info!(target:"citadel","[P2P-RECV-CONNECT] This stream will receive messages SENT BY peer {peer_cid}");
                            while let Some(message) = stream.next().await {
                                info!(target:"citadel","[P2P-RECV] Received P2P message! cid={cid}, peer_cid={peer_cid}, msg_len={}", message.len());
                                let message = InternalServiceResponse::MessageNotification(
                                    MessageNotification {
                                        message: message.into_buffer().into(),
                                        cid,
                                        peer_cid,
                                        request_id: Some(request_id),
                                    },
                                );

                                // Get the current associated TCP connection for this session (may have changed via ClaimSession)
                                let server_lock = server_conn_map.read();
                                let current_tcp_uuid = server_lock
                                    .get(&cid)
                                    .map(|conn| {
                                        conn.associated_localhost_connection
                                            .load(std::sync::atomic::Ordering::Relaxed)
                                    })
                                    .unwrap_or(uuid);
                                drop(server_lock);

                                info!(target:"citadel","[P2P-RECV] Forwarding to TCP uuid: {current_tcp_uuid}");

                                // First try the target connection directly
                                let tcp_map = hm_for_conn.read();
                                let mut sent_via_target = false;

                                if let Some(sender) = tcp_map.get(&current_tcp_uuid) {
                                    info!(target:"citadel","[P2P-RECV] Found TCP entry, sending MessageNotification");
                                    if sender.send(message.clone()).is_ok() {
                                        info!(target:"citadel","[P2P-RECV] Successfully sent MessageNotification to target client");
                                        sent_via_target = true;
                                    } else {
                                        error!(target:"citadel","[P2P-RECV] Error sending message to target client, will try fallback");
                                    }
                                }

                                // FALLBACK: If target not found or send failed, broadcast to all active connections
                                // This handles: TCP drop during send, race with ClaimSession, stale UUID
                                if !sent_via_target {
                                    warn!(target:"citadel","[P2P-RECV] Target TCP {} not found or send failed, broadcasting MessageNotification to all {} connections", current_tcp_uuid, tcp_map.len());

                                    let mut sent_count = 0;
                                    for (uuid, sender) in tcp_map.iter() {
                                        // Skip the connection we already tried (if any)
                                        if *uuid == current_tcp_uuid {
                                            continue;
                                        }
                                        if sender.send(message.clone()).is_ok() {
                                            sent_count += 1;
                                            info!(target:"citadel","[P2P-RECV] Broadcast sent to {}", uuid);
                                        }
                                    }

                                    if sent_count == 0 {
                                        warn!(target:"citadel","[P2P-RECV] No active connections for MessageNotification - message may be queued in ILM");
                                    } else {
                                        info!(target:"citadel","[P2P-RECV] Successfully broadcast MessageNotification to {} connections", sent_count);
                                    }
                                }

                                drop(tcp_map);
                            }
                            info!(target:"citadel","[P2P-RECV] P2P read stream ended for cid={cid} from peer={peer_cid}");
                        };

                        tokio::spawn(connection_read_stream);

                        InternalServiceResponse::PeerConnectSuccess(PeerConnectSuccess {
                            cid,
                            peer_cid,
                            request_id: Some(request_id),
                        })
                    }

                    Err(err) => {
                        let err_str = err.into_string();
                        error!(target: "citadel", "[PeerConnect] connect_to_peer_custom FAILED: {}", err_str);

                        InternalServiceResponse::PeerConnectFailure(PeerConnectFailure {
                            cid,
                            message: err_str,
                            request_id: Some(request_id),
                        })
                    }
                },
                Err(_elapsed) => {
                    error!(target: "citadel", "[PeerConnect] connect_to_peer_custom TIMED OUT after 30 seconds");
                    InternalServiceResponse::PeerConnectFailure(PeerConnectFailure {
                        cid,
                        message: "P2P connection timed out after 30 seconds".to_string(),
                        request_id: Some(request_id),
                    })
                }
            }
        }

        Err(err) => {
            let err_str = err.into_string();
            error!(target: "citadel", "[PeerConnect] find_target FAILED: {}", err_str);
            InternalServiceResponse::PeerConnectFailure(PeerConnectFailure {
                cid,
                message: err_str,
                request_id: Some(request_id),
            })
        }
    };

    Some(HandledRequestResult { response, uuid })
}

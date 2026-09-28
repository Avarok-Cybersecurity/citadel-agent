use crate::kernel::requests::peer::connect_read_stream::spawn_read_stream;
use crate::kernel::requests::peer::turn::{path_report, set_peer_turn};
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, PeerConnectFailure, PeerConnectSuccess,
};
use citadel_sdk::logging::{error, info};
use citadel_sdk::prefabs::ClientServerRemote;
use citadel_sdk::prelude::{
    ProtocolRemoteExt, ProtocolRemoteTargetExt, Ratchet, VirtualTargetType,
};
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
        turn,
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
                // "Assuming no peer" reaches the else branch below, which drops
                // the peer's sink from `conn.peers` -- and requests/message.rs
                // finds the peer through exactly that map, so a transient
                // stream error made an established P2P channel unreachable for
                // sending while both sides still believed it was up.
                info!(
                    target: "citadel",
                    "[PeerConnect] Failed to query SDK sessions: {:?}; refusing rather than \
                     dropping the peer channel",
                    e
                );
                return Some(HandledRequestResult {
                    response: InternalServiceResponse::PeerConnectFailure(PeerConnectFailure {
                        cid,
                        message: format!(
                            "Could not determine whether the channel to peer {} is still \
                             open: {:?}. Nothing was changed; try again.",
                            peer_cid, e
                        ),
                        request_id: Some(request_id),
                    }),
                    uuid,
                });
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

            // Before connecting: the peer's own PeerConnect / PeerConnectAccept supplies the
            // other half.
            if let Err(err) = set_peer_turn(remote, cid, peer_cid, turn.as_ref()).await {
                let err_str = err.into_string();
                error!(target: "citadel", "[PeerConnect] set_peer_turn FAILED: {}", err_str);
                return Some(HandledRequestResult {
                    response: InternalServiceResponse::PeerConnectFailure(PeerConnectFailure {
                        cid,
                        message: err_str,
                        request_id: Some(request_id),
                    }),
                    uuid,
                });
            }

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
                        let mut peer_connect_success = peer_connect_success;
                        // Taken before the channel is split and the struct is
                        // consumed. This is the only moment the UDP channel is
                        // offered; dropping it here would mean no call with this
                        // peer could ever use a datagram path.
                        let udp_rx = peer_connect_success.udp_channel_rx.take();
                        let path = path_report(peer_connect_success.channel.p2p_path());
                        info!(target: "citadel", "[PeerConnect] peer {} connected over {:?}", peer_cid, path);
                        let (sink, stream) = peer_connect_success.channel.split();
                        {
                            let mut map = this.server_connection_map.write();
                            if let Some(conn) = map.get_mut(&cid) {
                                conn.add_peer_connection(
                                    peer_cid,
                                    sink,
                                    peer_connect_success.remote,
                                    udp_rx,
                                );
                                info!(target: "citadel", "[PeerConnect] Added peer {} to cid {}'s peers. Total peers: {}", peer_cid, cid, conn.peers.len());
                            } else {
                                error!(target: "citadel", "[PeerConnect] CRITICAL: Cannot find session {} in server_connection_map to add peer {}", cid, peer_cid);
                            }
                        }

                        spawn_read_stream(this, cid, peer_cid, request_id, uuid, stream);

                        InternalServiceResponse::PeerConnectSuccess(PeerConnectSuccess {
                            cid,
                            peer_cid,
                            path,
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

use crate::kernel::peer_path::PathWatch;
use crate::kernel::requests::peer::turn::set_peer_turn;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::session_route::SessionRoute;
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
            info!(target: "citadel", "[PeerConnect] find_target succeeded, calling connect_to_peer_custom...");

            // Before connecting: the peer's own PeerConnect / PeerConnectAccept supplies the
            // other half.
            super::answer::remember_window_relay(this, cid, turn.as_ref());
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

            // No timeout of our own: the SDK delivers the channel as soon as it works over the
            // server relay (NAT traversal continues in the background), and bounds the wait for
            // the peer's answer itself. The 30s wrapper that stood here cut off connections that
            // were waiting on hole punching (PR #89 removed it alone; this supersedes that).
            match symmetric_identifier_handle_ref
                .connect_to_peer_custom(session_security_settings, udp_mode, peer_session_password)
                .await
            {
                Ok(peer_connect_success) => {
                    info!(target: "citadel", "[PeerConnect] connect_to_peer_custom succeeded!");
                    let mut peer_connect_success = peer_connect_success;
                    // Taken before the channel is split and the struct is
                    // consumed. This is the only moment the UDP channel is
                    // offered; dropping it here would mean no call with this
                    // peer could ever use a datagram path.
                    let udp_rx = peer_connect_success.udp_channel_rx.take();
                    // Subscribed before the path is read, so no change falls between the
                    // report below and the notifications that follow it.
                    let path_watch = PathWatch::new(&peer_connect_success.channel.p2p_path_cell());
                    let (path, upgrading) = path_watch.current();
                    info!(target: "citadel", "[PeerConnect] peer {} connected over {:?} (upgrading: {})", peer_cid, path, upgrading);
                    let (sink, mut stream) = peer_connect_success.channel.split();
                    let mut path_route = None;
                    {
                        let mut map = this.server_connection_map.write();
                        if let Some(conn) = map.get_mut(&cid) {
                            path_route = Some(SessionRoute::new(
                                conn.subscribers.clone(),
                                this.tx_to_localhost_clients.clone(),
                            ));
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

                    let read_route = path_route.clone();
                    let ilm = this.ilm_hosts.clone();

                    let connection_read_stream = async move {
                        info!(target:"citadel","[P2P-RECV-CONNECT] *** Starting P2P read stream for LOCAL_CID={cid} from PEER={peer_cid} ***");
                        info!(target:"citadel","[P2P-RECV-CONNECT] This stream will receive messages SENT BY peer {peer_cid}");
                        while let Some(message) = stream.next().await {
                            info!(target:"citadel","[P2P-RECV] Received P2P message! cid={cid}, peer_cid={peer_cid}, msg_len={}", message.len());
                            let arrived = MessageNotification {
                                message: message.into_buffer().into(),
                                cid,
                                peer_cid,
                                request_id: Some(request_id),
                            };
                            // A hosted account's ILM frames go to its ILM (kernel/ilm).
                            let Err(raw) = ilm.feed(arrived) else {
                                continue;
                            };
                            let message = InternalServiceResponse::MessageNotification(raw);

                            // To every connection attached to the session, and to
                            // nobody else. This once fell back to broadcasting to
                            // EVERY live TCP entry when the owner was stale, which
                            // handed the decrypted body of a P2P message to every
                            // other session multiplexed through this agent. The
                            // session's subscriber set, read at send time, is the
                            // sole authoritative destination; if nobody is attached,
                            // ILM is the layer that retries (kernel/session_route.rs).
                            let delivered = read_route
                                .as_ref()
                                .map(|route| route.send(message))
                                .unwrap_or_default();
                            if delivered.is_empty() {
                                warn!(target:"citadel","[P2P-RECV] No window attached to CID {cid}; ILM will retry");
                            } else {
                                info!(target:"citadel","[P2P-RECV] Delivered MessageNotification to {delivered:?}");
                            }
                        }
                        info!(target:"citadel","[P2P-RECV] P2P read stream ended for cid={cid} from peer={peer_cid}");
                    };

                    tokio::spawn(connection_read_stream);
                    if let Some(route) = path_route {
                        path_watch.forward(cid, peer_cid, route);
                    }

                    InternalServiceResponse::PeerConnectSuccess(PeerConnectSuccess {
                        cid,
                        peer_cid,
                        path,
                        upgrading,
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

use crate::kernel::requests::{handle_request, HandledRequestResult};
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, PeerRegisterFailure, PeerRegisterSuccess,
};
use citadel_sdk::logging::{error, info};
use citadel_sdk::prefabs::ClientServerRemote;
use citadel_sdk::prelude::{
    NodeRequest, ProtocolRemoteExt, ProtocolRemoteTargetExt, Ratchet, VirtualTargetType,
};
use futures::StreamExt;
use uuid::Uuid;

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::PeerRegister {
        request_id,
        cid,
        peer_cid,
        session_security_settings,
        connect_after_register,
        peer_session_password,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };

    info!(target: "citadel", "[PeerRegister] Received request: cid={}, peer_cid={}, connect_after_register={}, request_id={:?}", cid, peer_cid, connect_after_register, request_id);

    let remote = this.remote();

    // Check if peer is already registered before attempting registration
    // This prevents "Ratchet does not exist" errors from stale registration requests
    let already_registered = remote
        .account_manager()
        .get_hyperlan_peer_list(cid)
        .await
        .ok()
        .flatten()
        .map(|peers| peers.contains(&peer_cid))
        .unwrap_or(false);

    if already_registered {
        info!(target: "citadel", "[PeerRegister] Peer {} is already registered to {}, returning error", peer_cid, cid);
        return Some(HandledRequestResult {
            response: InternalServiceResponse::PeerRegisterFailure(PeerRegisterFailure {
                cid,
                message: format!("Peer {} is already registered", peer_cid),
                request_id: Some(request_id),
            }),
            uuid,
        });
    }

    let client_to_server_remote = ClientServerRemote::new(
        VirtualTargetType::LocalGroupServer { session_cid: cid },
        remote.clone(),
        session_security_settings,
        None,
        None,
    );

    // DEBUG: Query active sessions in the kernel's session_manager
    info!(target: "citadel", "[PeerRegister] Querying active sessions in session_manager...");
    match remote
        .send_callback_subscription(NodeRequest::GetActiveSessions)
        .await
    {
        Ok(mut stream) => {
            if let Some(result) = stream.next().await {
                info!(target: "citadel", "[PeerRegister] GetActiveSessions result: {:?}", result);
            }
        }
        Err(e) => {
            error!(target: "citadel", "[PeerRegister] Failed to query active sessions: {:?}", e);
        }
    }

    info!(target: "citadel", "[PeerRegister] Calling propose_target({}, {})...", cid, peer_cid);
    let response = match client_to_server_remote.propose_target(cid, peer_cid).await {
        Ok(symmetric_identifier_handle_ref) => {
            info!(target: "citadel", "[PeerRegister] propose_target succeeded, calling register_to_peer()...");
            match symmetric_identifier_handle_ref.register_to_peer().await {
                Ok(_peer_register_success) => {
                    info!(target: "citadel", "[PeerRegister] register_to_peer succeeded, getting account_manager...");
                    let account_manager = symmetric_identifier_handle_ref.account_manager();
                    info!(target: "citadel", "[PeerRegister] Calling find_target_information({}, {})...", cid, peer_cid);
                    match account_manager.find_target_information(cid, peer_cid).await {
                        Ok(target_information) => {
                            info!(target: "citadel", "[PeerRegister] find_target_information succeeded");
                            let (_, mutual_peer) = target_information.unwrap();
                            info!(target: "citadel", "[PeerRegister] mutual_peer.cid={}, connect_after_register={}", mutual_peer.cid, connect_after_register);

                            // Cache the peer's username for later use in ListRegisteredPeers
                            if let Some(ref username) = mutual_peer.username {
                                if !username.is_empty() {
                                    let mut cache = this.peer_username_cache.write();
                                    cache.insert((cid, mutual_peer.cid), username.clone());
                                    info!(target: "citadel", "[PeerRegister] Cached username '{}' for peer {} (session {})", username, mutual_peer.cid, cid);
                                }
                            }

                            match connect_after_register {
                                true => {
                                    info!(target: "citadel", "[PeerRegister] connect_after_register=true, sending PeerRegisterSuccess first, then chaining to PeerConnect...");

                                    // CRITICAL: Send PeerRegisterSuccess FIRST so the initiator's frontend
                                    // knows the peer is registered. Without this, the frontend only receives
                                    // PeerConnectSuccess and doesn't update its peer store.
                                    let register_success = InternalServiceResponse::PeerRegisterSuccess(
                                        PeerRegisterSuccess {
                                            cid,
                                            peer_cid: mutual_peer.cid,
                                            peer_username: mutual_peer
                                                .username
                                                .clone()
                                                .unwrap_or_default(),
                                            request_id: Some(request_id),
                                        },
                                    );

                                    // Send PeerRegisterSuccess to the initiator via TCP
                                    {
                                        let tcp_map = this.tx_to_localhost_clients.read();
                                        if let Some(sender) = tcp_map.get(&uuid) {
                                            if let Err(e) = sender.send(register_success) {
                                                error!(target: "citadel", "[PeerRegister] Failed to send PeerRegisterSuccess: {:?}", e);
                                            } else {
                                                info!(target: "citadel", "[PeerRegister] Sent PeerRegisterSuccess to initiator before PeerConnect chain");
                                            }
                                        }
                                    }

                                    // Now chain to PeerConnect
                                    let connect_command = InternalServiceRequest::PeerConnect {
                                        cid,
                                        peer_cid: mutual_peer.cid,
                                        udp_mode: Default::default(),
                                        session_security_settings,
                                        request_id,
                                        peer_session_password,
                                    };

                                    let result = handle_request(this, uuid, connect_command).await;
                                    info!(target: "citadel", "[PeerRegister] PeerConnect chain returned: {:?}", result.is_some());
                                    return result;
                                }
                                false => {
                                    info!(target: "citadel", "[PeerRegister] connect_after_register=false, returning PeerRegisterSuccess");
                                    InternalServiceResponse::PeerRegisterSuccess(
                                        PeerRegisterSuccess {
                                            cid,
                                            peer_cid: mutual_peer.cid,
                                            peer_username: mutual_peer
                                                .username
                                                .clone()
                                                .unwrap_or_default(),
                                            request_id: Some(request_id),
                                        },
                                    )
                                }
                            }
                        }
                        Err(err) => {
                            let err_str = err.into_string();
                            error!(target: "citadel", "[PeerRegister] find_target_information FAILED: {}", err_str);
                            InternalServiceResponse::PeerRegisterFailure(PeerRegisterFailure {
                                cid,
                                message: err_str,
                                request_id: Some(request_id),
                            })
                        }
                    }
                }

                Err(err) => {
                    let err_str = err.into_string();
                    error!(target: "citadel", "[PeerRegister] register_to_peer FAILED: {}", err_str);
                    InternalServiceResponse::PeerRegisterFailure(PeerRegisterFailure {
                        cid,
                        message: err_str,
                        request_id: Some(request_id),
                    })
                }
            }
        }

        Err(err) => {
            let err_str = err.into_string();
            error!(target: "citadel", "[PeerRegister] propose_target FAILED: {}", err_str);
            InternalServiceResponse::PeerRegisterFailure(PeerRegisterFailure {
                cid,
                message: err_str,
                request_id: Some(request_id),
            })
        }
    };

    info!(target: "citadel", "[PeerRegister] Returning response for cid={}, peer_cid={}", cid, peer_cid);
    Some(HandledRequestResult { response, uuid })
}

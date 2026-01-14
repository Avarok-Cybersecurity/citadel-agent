use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::{
        get_free_port, register_and_connect_to_server, server_info_skip_cert_verification,
        spawn_services, InternalServicesFutures, RegisterAndConnectItems,
    };
    use citadel_internal_service::kernel::CitadelWorkspaceService;
    use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
    use citadel_sdk::logging::info;
    use citadel_sdk::prelude::*;
    use std::error::Error;
    use std::net::SocketAddr;
    use std::time::Duration;
    use uuid::Uuid;

    /// Test that ListAllPeers returns all peers on the network (not just registered ones)
    #[tokio::test]
    async fn test_list_all_peers() -> Result<(), Box<dyn Error>> {
        crate::common::setup_log();
        info!(target: "citadel", "=== Starting test_list_all_peers ===");

        // Create server
        let (server, server_bind_address) = server_info_skip_cert_verification::<StackedRatchet>();
        tokio::task::spawn(server);

        // Create two internal services (one for each peer)
        let bind_addr_1: SocketAddr = format!("127.0.0.1:{}", get_free_port()).parse().unwrap();
        let bind_addr_2: SocketAddr = format!("127.0.0.1:{}", get_free_port()).parse().unwrap();

        let internal_service_kernel_1 =
            CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(bind_addr_1).await?;
        let internal_service_1 = NodeBuilder::default()
            .with_node_type(NodeType::Peer)
            .with_backend(BackendType::InMemory)
            .with_insecure_skip_cert_verification()
            .build(internal_service_kernel_1)?;

        let internal_service_kernel_2 =
            CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(bind_addr_2).await?;
        let internal_service_2 = NodeBuilder::default()
            .with_node_type(NodeType::Peer)
            .with_backend(BackendType::InMemory)
            .with_insecure_skip_cert_verification()
            .build(internal_service_kernel_2)?;

        let mut internal_services: Vec<InternalServicesFutures> = Vec::new();
        internal_services.push(Box::pin(async move {
            match internal_service_1.await {
                Err(err) => Err(Box::from(err)),
                _ => Ok(()),
            }
        }));
        internal_services.push(Box::pin(async move {
            match internal_service_2.await {
                Err(err) => Err(Box::from(err)),
                _ => Ok(()),
            }
        }));
        spawn_services(internal_services);

        // Wait for services to start
        tokio::time::sleep(Duration::from_millis(2000)).await;

        // Register and connect two users (but don't register them as P2P peers)
        let to_spawn = vec![
            RegisterAndConnectItems {
                internal_service_addr: bind_addr_1,
                server_addr: server_bind_address,
                full_name: "Alice User",
                username: "alice",
                password: "secret_alice",
                pre_shared_key: None::<PreSharedKey>,
            },
            RegisterAndConnectItems {
                internal_service_addr: bind_addr_2,
                server_addr: server_bind_address,
                full_name: "Bob User",
                username: "bob",
                password: "secret_bob",
                pre_shared_key: None::<PreSharedKey>,
            },
        ];

        let mut service_handles = register_and_connect_to_server(to_spawn).await?;
        let (to_service_a, mut from_service_a, cid_a) = service_handles.remove(0);
        let (_to_service_b, mut _from_service_b, cid_b) = service_handles.remove(0);

        info!(target: "citadel", "Alice CID: {}, Bob CID: {}", cid_a, cid_b);

        // Wait for sessions to stabilize
        tokio::time::sleep(Duration::from_millis(2000)).await;

        // Alice sends ListAllPeers request
        let request_id = Uuid::new_v4();
        to_service_a
            .send(InternalServiceRequest::ListAllPeers {
                cid: cid_a,
                request_id,
            })
            .unwrap();

        info!(target: "citadel", "Sent ListAllPeers request from Alice");

        // Wait for response (with timeout)
        let response = tokio::time::timeout(Duration::from_secs(10), from_service_a.recv())
            .await
            .expect("Timeout waiting for ListAllPeers response")
            .expect("Channel closed");

        match response {
            InternalServiceResponse::ListAllPeersResponse(resp) => {
                info!(target: "citadel", "ListAllPeers SUCCESS: Found {} peers", resp.peer_information.len());

                // Should find at least Bob (and possibly others)
                // The response should NOT include Alice herself
                assert!(
                    !resp.peer_information.contains_key(&cid_a),
                    "ListAllPeers should not include the requesting peer (self)"
                );

                // Bob should be in the list (they are on the same network)
                assert!(
                    resp.peer_information.contains_key(&cid_b),
                    "ListAllPeers should include Bob who is on the same network. Found: {:?}",
                    resp.peer_information.keys().collect::<Vec<_>>()
                );

                // Verify Bob's information
                let bob_info = resp.peer_information.get(&cid_b).unwrap();
                assert_eq!(bob_info.username, Some("bob".to_string()));
                assert_eq!(bob_info.name, Some("Bob User".to_string()));

                info!(target: "citadel", "ListAllPeers test PASSED");
            }
            InternalServiceResponse::ListAllPeersFailure(failure) => {
                panic!("ListAllPeers failed: {}", failure.message);
            }
            other => {
                panic!("Unexpected response: {:?}", other);
            }
        }

        Ok(())
    }

    /// Test that ListRegisteredPeers returns ONLY peers that have been P2P registered
    #[tokio::test]
    async fn test_list_registered_peers() -> Result<(), Box<dyn Error>> {
        crate::common::setup_log();
        info!(target: "citadel", "=== Starting test_list_registered_peers ===");

        // Create server
        let (server, server_bind_address) = server_info_skip_cert_verification::<StackedRatchet>();
        tokio::task::spawn(server);

        // Create two internal services
        let bind_addr_1: SocketAddr = format!("127.0.0.1:{}", get_free_port()).parse().unwrap();
        let bind_addr_2: SocketAddr = format!("127.0.0.1:{}", get_free_port()).parse().unwrap();

        let internal_service_kernel_1 =
            CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(bind_addr_1).await?;
        let internal_service_1 = NodeBuilder::default()
            .with_node_type(NodeType::Peer)
            .with_backend(BackendType::InMemory)
            .with_insecure_skip_cert_verification()
            .build(internal_service_kernel_1)?;

        let internal_service_kernel_2 =
            CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(bind_addr_2).await?;
        let internal_service_2 = NodeBuilder::default()
            .with_node_type(NodeType::Peer)
            .with_backend(BackendType::InMemory)
            .with_insecure_skip_cert_verification()
            .build(internal_service_kernel_2)?;

        let mut internal_services: Vec<InternalServicesFutures> = Vec::new();
        internal_services.push(Box::pin(async move {
            match internal_service_1.await {
                Err(err) => Err(Box::from(err)),
                _ => Ok(()),
            }
        }));
        internal_services.push(Box::pin(async move {
            match internal_service_2.await {
                Err(err) => Err(Box::from(err)),
                _ => Ok(()),
            }
        }));
        spawn_services(internal_services);

        tokio::time::sleep(Duration::from_millis(2000)).await;

        // Register and connect two users
        let to_spawn = vec![
            RegisterAndConnectItems {
                internal_service_addr: bind_addr_1,
                server_addr: server_bind_address,
                full_name: "Charlie User",
                username: "charlie",
                password: "secret_charlie",
                pre_shared_key: None::<PreSharedKey>,
            },
            RegisterAndConnectItems {
                internal_service_addr: bind_addr_2,
                server_addr: server_bind_address,
                full_name: "Dave User",
                username: "dave",
                password: "secret_dave",
                pre_shared_key: None::<PreSharedKey>,
            },
        ];

        let mut service_handles = register_and_connect_to_server(to_spawn).await?;
        let (to_service_a, mut from_service_a, cid_a) = service_handles.remove(0);
        let (_to_service_b, mut _from_service_b, cid_b) = service_handles.remove(0);

        info!(target: "citadel", "Charlie CID: {}, Dave CID: {}", cid_a, cid_b);

        tokio::time::sleep(Duration::from_millis(2000)).await;

        // First, test ListRegisteredPeers BEFORE P2P registration (should be empty)
        let request_id = Uuid::new_v4();
        to_service_a
            .send(InternalServiceRequest::ListRegisteredPeers {
                cid: cid_a,
                request_id,
            })
            .unwrap();

        info!(target: "citadel", "Sent ListRegisteredPeers request (before P2P registration)");

        let response = tokio::time::timeout(Duration::from_secs(10), from_service_a.recv())
            .await
            .expect("Timeout waiting for ListRegisteredPeers response")
            .expect("Channel closed");

        match response {
            InternalServiceResponse::ListRegisteredPeersResponse(resp) => {
                info!(target: "citadel", "ListRegisteredPeers (before registration): Found {} peers", resp.peers.len());

                // Before P2P registration, should have NO registered peers
                assert!(
                    resp.peers.is_empty(),
                    "ListRegisteredPeers should return empty before P2P registration. Found: {:?}",
                    resp.peers.keys().collect::<Vec<_>>()
                );

                info!(target: "citadel", "ListRegisteredPeers (before registration) test PASSED");
            }
            InternalServiceResponse::ListRegisteredPeersFailure(failure) => {
                panic!("ListRegisteredPeers failed: {}", failure.message);
            }
            other => {
                panic!("Unexpected response: {:?}", other);
            }
        }

        Ok(())
    }

    /// Test that ListRegisteredPeers returns registered peers AFTER P2P registration
    #[tokio::test]
    async fn test_list_registered_peers_after_registration() -> Result<(), Box<dyn Error>> {
        crate::common::setup_log();
        info!(target: "citadel", "=== Starting test_list_registered_peers_after_registration ===");

        // Create server
        let (server, server_bind_address) = server_info_skip_cert_verification::<StackedRatchet>();
        tokio::task::spawn(server);

        // Create two internal services
        let bind_addr_1: SocketAddr = format!("127.0.0.1:{}", get_free_port()).parse().unwrap();
        let bind_addr_2: SocketAddr = format!("127.0.0.1:{}", get_free_port()).parse().unwrap();

        let internal_service_kernel_1 =
            CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(bind_addr_1).await?;
        let internal_service_1 = NodeBuilder::default()
            .with_node_type(NodeType::Peer)
            .with_backend(BackendType::InMemory)
            .with_insecure_skip_cert_verification()
            .build(internal_service_kernel_1)?;

        let internal_service_kernel_2 =
            CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(bind_addr_2).await?;
        let internal_service_2 = NodeBuilder::default()
            .with_node_type(NodeType::Peer)
            .with_backend(BackendType::InMemory)
            .with_insecure_skip_cert_verification()
            .build(internal_service_kernel_2)?;

        let mut internal_services: Vec<InternalServicesFutures> = Vec::new();
        internal_services.push(Box::pin(async move {
            match internal_service_1.await {
                Err(err) => Err(Box::from(err)),
                _ => Ok(()),
            }
        }));
        internal_services.push(Box::pin(async move {
            match internal_service_2.await {
                Err(err) => Err(Box::from(err)),
                _ => Ok(()),
            }
        }));
        spawn_services(internal_services);

        tokio::time::sleep(Duration::from_millis(2000)).await;

        // Register and connect two users
        let to_spawn = vec![
            RegisterAndConnectItems {
                internal_service_addr: bind_addr_1,
                server_addr: server_bind_address,
                full_name: "Eve User",
                username: "eve",
                password: "secret_eve",
                pre_shared_key: None::<PreSharedKey>,
            },
            RegisterAndConnectItems {
                internal_service_addr: bind_addr_2,
                server_addr: server_bind_address,
                full_name: "Frank User",
                username: "frank",
                password: "secret_frank",
                pre_shared_key: None::<PreSharedKey>,
            },
        ];

        let mut service_handles = register_and_connect_to_server(to_spawn).await?;
        let (to_service_a, mut from_service_a, cid_a) = service_handles.remove(0);
        let (to_service_b, mut from_service_b, cid_b) = service_handles.remove(0);

        info!(target: "citadel", "Eve CID: {}, Frank CID: {}", cid_a, cid_b);

        tokio::time::sleep(Duration::from_millis(2000)).await;

        // Now perform P2P registration between Eve and Frank
        let session_security_settings = SessionSecuritySettingsBuilder::default().build()?;

        // Eve sends PeerRegister to Frank
        to_service_a
            .send(InternalServiceRequest::PeerRegister {
                request_id: Uuid::new_v4(),
                cid: cid_a,
                peer_cid: cid_b,
                session_security_settings,
                connect_after_register: false,
                peer_session_password: None,
            })
            .unwrap();

        info!(target: "citadel", "Eve sent PeerRegister to Frank");

        // Frank receives PeerRegisterNotification
        let notification = tokio::time::timeout(Duration::from_secs(10), from_service_b.recv())
            .await
            .expect("Timeout waiting for PeerRegisterNotification")
            .expect("Channel closed");

        match notification {
            InternalServiceResponse::PeerRegisterNotification(notif) => {
                info!(target: "citadel", "Frank received PeerRegisterNotification from Eve (peer_cid={})", notif.peer_cid);
                assert_eq!(notif.peer_cid, cid_a);
            }
            other => panic!("Expected PeerRegisterNotification, got {:?}", other),
        }

        // Frank accepts by sending PeerRegister back
        to_service_b
            .send(InternalServiceRequest::PeerRegister {
                request_id: Uuid::new_v4(),
                cid: cid_b,
                peer_cid: cid_a,
                session_security_settings,
                connect_after_register: false,
                peer_session_password: None,
            })
            .unwrap();

        info!(target: "citadel", "Frank accepted PeerRegister");

        // Both should receive PeerRegisterSuccess
        let eve_success = tokio::time::timeout(Duration::from_secs(10), from_service_a.recv())
            .await
            .expect("Timeout waiting for Eve's PeerRegisterSuccess")
            .expect("Channel closed");

        match eve_success {
            InternalServiceResponse::PeerRegisterSuccess(success) => {
                info!(target: "citadel", "Eve received PeerRegisterSuccess (peer_cid={})", success.peer_cid);
                assert_eq!(success.peer_cid, cid_b);
            }
            other => panic!("Expected PeerRegisterSuccess for Eve, got {:?}", other),
        }

        let frank_success = tokio::time::timeout(Duration::from_secs(10), from_service_b.recv())
            .await
            .expect("Timeout waiting for Frank's PeerRegisterSuccess")
            .expect("Channel closed");

        match frank_success {
            InternalServiceResponse::PeerRegisterSuccess(success) => {
                info!(target: "citadel", "Frank received PeerRegisterSuccess (peer_cid={})", success.peer_cid);
                assert_eq!(success.peer_cid, cid_a);
            }
            other => panic!("Expected PeerRegisterSuccess for Frank, got {:?}", other),
        }

        // Wait for registration to propagate
        tokio::time::sleep(Duration::from_millis(2000)).await;

        // Now test ListRegisteredPeers - should find Frank
        let request_id = Uuid::new_v4();
        to_service_a
            .send(InternalServiceRequest::ListRegisteredPeers {
                cid: cid_a,
                request_id,
            })
            .unwrap();

        info!(target: "citadel", "Sent ListRegisteredPeers request (after P2P registration)");

        let response = tokio::time::timeout(Duration::from_secs(10), from_service_a.recv())
            .await
            .expect("Timeout waiting for ListRegisteredPeers response")
            .expect("Channel closed");

        match response {
            InternalServiceResponse::ListRegisteredPeersResponse(resp) => {
                info!(target: "citadel", "ListRegisteredPeers (after registration): Found {} peers", resp.peers.len());

                // After P2P registration, should have Frank as a registered peer
                assert!(
                    resp.peers.contains_key(&cid_b),
                    "ListRegisteredPeers should return Frank after P2P registration. Found: {:?}",
                    resp.peers.keys().collect::<Vec<_>>()
                );

                info!(target: "citadel", "ListRegisteredPeers (after registration) test PASSED");
            }
            InternalServiceResponse::ListRegisteredPeersFailure(failure) => {
                panic!("ListRegisteredPeers failed: {}", failure.message);
            }
            other => {
                panic!("Unexpected response: {:?}", other);
            }
        }

        Ok(())
    }
}

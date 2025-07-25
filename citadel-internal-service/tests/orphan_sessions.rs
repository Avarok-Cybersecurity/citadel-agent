use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::{
        get_free_port, register_and_connect_to_server as register_multiple, 
        server_info_skip_cert_verification, RegisterAndConnectItems,
    };
    use citadel_internal_service::kernel::CitadelWorkspaceService;
    use citadel_internal_service_types::{
        ConfigCommand, InternalServiceRequest, InternalServiceResponse,
    };
    use citadel_sdk::prelude::*;
    use std::error::Error;
    use std::net::SocketAddr;
    use std::time::Duration;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_orphan_mode_enable_disable() -> Result<(), Box<dyn Error>> {
        crate::common::setup_log();
        let bind_address_internal_service: SocketAddr =
            format!("127.0.0.1:{}", get_free_port()).parse().unwrap();

        let (server, server_bind_address) = server_info_skip_cert_verification::<StackedRatchet>();
        tokio::task::spawn(server);

        let internal_service_kernel =
            CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(bind_address_internal_service)
                .await?;
        let internal_service = NodeBuilder::default()
            .with_node_type(NodeType::Peer)
            .with_backend(BackendType::InMemory)
            .with_insecure_skip_cert_verification()
            .build(internal_service_kernel)?;

        tokio::task::spawn(internal_service);
        tokio::time::sleep(Duration::from_millis(2000)).await;

        // Register and connect to server
        let to_spawn = vec![RegisterAndConnectItems {
            internal_service_addr: bind_address_internal_service,
            server_addr: server_bind_address,
            full_name: "Test User",
            username: "test.user",
            password: "password",
            pre_shared_key: None::<PreSharedKey>,
        }];
        
        let mut service_vec = register_multiple(to_spawn).await?;
        let (to_service, mut from_service, _cid) = service_vec.remove(0);

        // Enable orphan mode
        let request_id = Uuid::new_v4();
        let enable_orphan_request = InternalServiceRequest::ConnectionManagement {
            request_id,
            management_command: ConfigCommand::SetConnectionOrphan {
                allow_orphan_sessions: true,
            },
        };

        to_service.send(enable_orphan_request)?;

        // Wait for response
        let response = from_service.recv().await.ok_or("No response received")?;
        
        match response {
            InternalServiceResponse::ConnectionManagementSuccess(success) => {
                assert_eq!(success.message, "Orphan mode enabled for connection");
            }
            _ => panic!("Expected ConnectionManagementSuccess, got {:?}", response),
        }

        // Disable orphan mode
        let request_id = Uuid::new_v4();
        let disable_orphan_request = InternalServiceRequest::ConnectionManagement {
            request_id,
            management_command: ConfigCommand::SetConnectionOrphan {
                allow_orphan_sessions: false,
            },
        };

        to_service.send(disable_orphan_request)?;

        let response = from_service.recv().await.ok_or("No response received")?;
        
        match response {
            InternalServiceResponse::ConnectionManagementSuccess(success) => {
                assert_eq!(success.message, "Orphan mode disabled for connection");
            }
            _ => panic!("Expected ConnectionManagementSuccess, got {:?}", response),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_orphan_session_persistence() -> Result<(), Box<dyn Error>> {
        crate::common::setup_log();
        
        let bind_address_internal_service: SocketAddr =
            format!("127.0.0.1:{}", get_free_port()).parse().unwrap();

        let (server, server_bind_address) = server_info_skip_cert_verification::<StackedRatchet>();
        tokio::task::spawn(server);

        let internal_service_kernel =
            CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(bind_address_internal_service)
                .await?;
        let internal_service = NodeBuilder::default()
            .with_node_type(NodeType::Peer)
            .with_backend(BackendType::InMemory)
            .with_insecure_skip_cert_verification()
            .build(internal_service_kernel)?;

        tokio::task::spawn(internal_service);
        tokio::time::sleep(Duration::from_millis(2000)).await;

        // First connection - register and connect
        let to_spawn = vec![RegisterAndConnectItems {
            internal_service_addr: bind_address_internal_service,
            server_addr: server_bind_address,
            full_name: "Orphan Test User",
            username: "orphan.test",
            password: "password123",
            pre_shared_key: None::<PreSharedKey>,
        }];
        
        let mut service_vec = register_multiple(to_spawn).await?;
        let (to_service, mut from_service, session_cid) = service_vec.remove(0);

        // Enable orphan mode
        let request_id = Uuid::new_v4();
        let enable_orphan_request = InternalServiceRequest::ConnectionManagement {
            request_id,
            management_command: ConfigCommand::SetConnectionOrphan {
                allow_orphan_sessions: true,
            },
        };

        to_service.send(enable_orphan_request)?;
        let _ = from_service.recv().await.ok_or("No response received")?;

        // Disconnect first connection
        drop(to_service);
        drop(from_service);
        tokio::time::sleep(Duration::from_millis(1000)).await;

        // Second connection - should be able to claim the orphaned session
        let to_spawn2 = vec![RegisterAndConnectItems {
            internal_service_addr: bind_address_internal_service,
            server_addr: server_bind_address,
            full_name: "Second User",
            username: "second.user",
            password: "password456",
            pre_shared_key: None::<PreSharedKey>,
        }];
        
        let mut service_vec2 = register_multiple(to_spawn2).await?;
        let (to_service2, mut from_service2, _cid2) = service_vec2.remove(0);

        // Try to claim the orphaned session
        let request_id = Uuid::new_v4();
        let claim_request = InternalServiceRequest::ConnectionManagement {
            request_id,
            management_command: ConfigCommand::ClaimSession {
                session_cid,
                only_if_orphaned: true,
            },
        };

        to_service2.send(claim_request)?;
        let response = from_service2.recv().await.ok_or("No response received")?;

        match response {
            InternalServiceResponse::ConnectionManagementSuccess(success) => {
                assert!(success.message.contains("Successfully claimed session"));
            }
            _ => panic!("Expected ConnectionManagementSuccess, got {:?}", response),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_disconnect_all_orphan_sessions() -> Result<(), Box<dyn Error>> {
        crate::common::setup_log();
        
        let bind_address_internal_service: SocketAddr =
            format!("127.0.0.1:{}", get_free_port()).parse().unwrap();

        let (server, server_bind_address) = server_info_skip_cert_verification::<StackedRatchet>();
        tokio::task::spawn(server);

        let internal_service_kernel =
            CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(bind_address_internal_service)
                .await?;
        let internal_service = NodeBuilder::default()
            .with_node_type(NodeType::Peer)
            .with_backend(BackendType::InMemory)
            .with_insecure_skip_cert_verification()
            .build(internal_service_kernel)?;

        tokio::task::spawn(internal_service);
        tokio::time::sleep(Duration::from_millis(2000)).await;

        // Create an orphaned session
        let to_spawn = vec![RegisterAndConnectItems {
            internal_service_addr: bind_address_internal_service,
            server_addr: server_bind_address,
            full_name: "Orphan User 1",
            username: "orphan.user1",
            password: "password123",
            pre_shared_key: None::<PreSharedKey>,
        }];
        
        let mut service_vec = register_multiple(to_spawn).await?;
        let (to_service, mut from_service, _cid) = service_vec.remove(0);

        // Enable orphan mode
        let request_id = Uuid::new_v4();
        let enable_orphan_request = InternalServiceRequest::ConnectionManagement {
            request_id,
            management_command: ConfigCommand::SetConnectionOrphan {
                allow_orphan_sessions: true,
            },
        };

        to_service.send(enable_orphan_request)?;
        let _ = from_service.recv().await.ok_or("No response received")?;

        // Disconnect to create orphan
        drop(to_service);
        drop(from_service);
        tokio::time::sleep(Duration::from_millis(1000)).await;

        // New connection to manage orphans
        let to_spawn2 = vec![RegisterAndConnectItems {
            internal_service_addr: bind_address_internal_service,
            server_addr: server_bind_address,
            full_name: "Admin User",
            username: "admin.user",
            password: "adminpass",
            pre_shared_key: None::<PreSharedKey>,
        }];
        
        let mut service_vec2 = register_multiple(to_spawn2).await?;
        let (to_service2, mut from_service2, _cid2) = service_vec2.remove(0);

        // Disconnect all orphan sessions
        let request_id = Uuid::new_v4();
        let disconnect_all_request = InternalServiceRequest::ConnectionManagement {
            request_id,
            management_command: ConfigCommand::DisconnectOrphan {
                session_cid: None,
            },
        };

        to_service2.send(disconnect_all_request)?;
        let response = from_service2.recv().await.ok_or("No response received")?;

        match response {
            InternalServiceResponse::ConnectionManagementSuccess(success) => {
                assert!(success.message.contains("Disconnected") && success.message.contains("orphan sessions"));
            }
            _ => panic!("Expected ConnectionManagementSuccess, got {:?}", response),
        }

        Ok(())
    }
}
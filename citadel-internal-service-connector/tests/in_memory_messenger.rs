#[cfg(test)]
mod tests {
    use citadel_internal_service_connector::connector::InternalServiceConnector;
    use citadel_internal_service_connector::io_interface::in_memory::InMemoryInterface;
    use citadel_internal_service_connector::io_interface::IOInterface;
    use citadel_internal_service_types::{
        InternalServicePayload, InternalServiceRequest, InternalServiceResponse,
    };
    use futures::{SinkExt, StreamExt};
    use tokio::sync::mpsc;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_in_memory_interface_creation() {
        let (tx, rx) = mpsc::unbounded_channel::<InternalServicePayload>();

        let interface = InMemoryInterface {
            sink: Some(tx),
            stream: Some(rx),
        };

        assert!(interface.sink.is_some());
        assert!(interface.stream.is_some());
    }

    #[tokio::test]
    async fn test_in_memory_connector_basic() {
        // Create two pairs of channels to simulate two services communicating
        let (tx_a_to_b, mut rx_a_to_b) = mpsc::unbounded_channel::<InternalServicePayload>();
        let (tx_b_to_a, rx_b_to_a) = mpsc::unbounded_channel::<InternalServicePayload>();

        // Create interface A that sends to B
        let interface_a = InMemoryInterface {
            sink: Some(tx_a_to_b.clone()),
            stream: Some(rx_b_to_a),
        };

        // Create connector A
        let connector_a = InternalServiceConnector::from_io(interface_a)
            .await
            .expect("Failed to create connector A");

        let (mut sink_a, _stream_a) = (connector_a.sink, connector_a.stream);

        // Test sending a connect request
        let request_id = Uuid::new_v4();
        let connect_request = InternalServiceRequest::Connect {
            username: "test_user".to_string(),
            password: vec![].into(), // SecBuffer
            connect_mode: Default::default(),
            udp_mode: Default::default(),
            keep_alive_timeout: None,
            session_security_settings: Default::default(),
            server_password: None,
            request_id,
        };

        sink_a.send(connect_request).await.unwrap();

        // Check that the message was sent through the channel
        let received = rx_a_to_b.recv().await;
        assert!(received.is_some());

        if let Some(InternalServicePayload::Request(req)) = received {
            match req {
                InternalServiceRequest::Connect { username, .. } => {
                    assert_eq!(username, "test_user");
                }
                _ => panic!("Expected Connect request"),
            }
        } else {
            panic!("Expected Request payload");
        }
    }

    #[tokio::test]
    async fn test_from_request_response_pair() {
        // Create channels that simulate the internal service interface
        let (tx_request, mut rx_request) = mpsc::unbounded_channel::<InternalServiceRequest>();
        let (tx_response, rx_response) = mpsc::unbounded_channel::<InternalServiceResponse>();

        // Create interface using from_request_response_pair
        let interface = InMemoryInterface::from_request_response_pair(tx_request, rx_response);
        let connector = InternalServiceConnector::from_io(interface).await.unwrap();
        let (mut sink, mut stream) = (connector.sink, connector.stream);

        // Test connect request
        let request_id = Uuid::new_v4();
        let connect_req = InternalServiceRequest::Connect {
            username: "test".to_string(),
            password: vec![].into(),
            connect_mode: Default::default(),
            udp_mode: Default::default(),
            keep_alive_timeout: None,
            session_security_settings: Default::default(),
            server_password: None,
            request_id,
        };

        // Send request
        sink.send(connect_req).await.unwrap();

        // Verify it was received
        let received_req = rx_request.recv().await.unwrap();
        match received_req {
            InternalServiceRequest::Connect {
                username,
                request_id: req_id,
                ..
            } => {
                assert_eq!(username, "test");
                assert_eq!(req_id, request_id);

                // Send response back
                use citadel_internal_service_types::ConnectSuccess;
                let response = InternalServiceResponse::ConnectSuccess(ConnectSuccess {
                    cid: 1234,
                    request_id: Some(request_id),
                });
                tx_response.send(response).unwrap();
            }
            _ => panic!("Expected Connect request"),
        }

        // Verify response is received
        if let Some(InternalServiceResponse::ConnectSuccess(success)) = stream.next().await {
            assert_eq!(success.cid, 1234);
            assert_eq!(success.request_id, Some(request_id));
        } else {
            panic!("Expected ConnectSuccess response");
        }
    }

    // Disabled for WASM due to tokio runtime parking issues
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn test_sink_and_stream_work() {
        // Create a direct pipe without spawning tasks (WASM-friendly)
        let (tx1, _rx1) = mpsc::unbounded_channel::<InternalServicePayload>();
        let (tx2, rx2) = mpsc::unbounded_channel::<InternalServicePayload>();

        // Interface that receives from rx2 and sends to tx1
        let interface = InMemoryInterface {
            sink: Some(tx1),
            stream: Some(rx2),
        };

        let connector = InternalServiceConnector::from_io(interface).await.unwrap();
        let (mut sink, mut stream) = (connector.sink, connector.stream);

        // Send a request
        let req = InternalServiceRequest::LocalDBGetKV {
            request_id: Uuid::new_v4(),
            cid: 1,
            peer_cid: None,
            key: "test_key".to_string(),
        };
        sink.send(req.clone()).await.unwrap();

        // The message should be available in rx1
        // Simulate a response by sending to tx2
        let response = InternalServiceResponse::LocalDBGetKVSuccess(
            citadel_internal_service_types::LocalDBGetKVSuccess {
                cid: 1,
                peer_cid: None,
                key: "test_key".to_string(),
                value: vec![1, 2, 3],
                request_id: None,
            },
        );
        tx2.send(InternalServicePayload::Response(response))
            .unwrap();

        // Should receive the response
        if let Some(resp) = stream.next().await {
            match resp {
                InternalServiceResponse::LocalDBGetKVSuccess(kv_resp) => {
                    assert_eq!(kv_resp.cid, 1);
                    assert_eq!(kv_resp.value, vec![1, 2, 3]);
                }
                _ => panic!("Expected LocalDBGetKVSuccess"),
            }
        } else {
            panic!("Expected to receive response");
        }
    }
}

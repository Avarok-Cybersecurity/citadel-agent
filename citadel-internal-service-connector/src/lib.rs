pub mod codec;
pub mod connector;
pub mod io_interface;
pub mod messenger;

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use crate::connector::InternalServiceConnector;
    use crate::io_interface::in_memory::InMemoryInterface;
    use crate::io_interface::IOInterface;
    use citadel_internal_service_types::{
        InternalServicePayload, InternalServiceRequest, InternalServiceResponse,
    };
    use citadel_io::tokio;
    use citadel_io::tokio::sync::mpsc;
    use futures::{SinkExt, StreamExt};

    #[tokio::test]
    async fn test_in_memory_connector() {
        // Create channels for communication
        let (tx1, rx1) = mpsc::unbounded_channel::<InternalServicePayload>();
        let (tx2, rx2) = mpsc::unbounded_channel::<InternalServicePayload>();

        // Create in-memory interface
        let interface = InMemoryInterface {
            sink: Some(tx1),
            stream: Some(rx2),
        };

        // Create connector
        let connector = InternalServiceConnector::from_io(interface).await.unwrap();
        let (mut sink, mut stream) = (connector.sink, connector.stream);

        // Create another interface for the other end
        let mut other_interface = InMemoryInterface {
            sink: Some(tx2),
            stream: Some(rx1),
        };

        // Send a request
        let request = InternalServiceRequest::GetSessions {
            request_id: uuid::Uuid::new_v4(),
        };
        sink.send(request.clone()).await.unwrap();

        // Receive on the other end
        if let Some(InternalServicePayload::Request(received)) =
            other_interface.stream.as_mut().unwrap().recv().await
        {
            // Send response back
            let response = InternalServiceResponse::GetSessionsResponse(
                citadel_internal_service_types::GetSessionsResponse {
                    cid: 1,
                    sessions: vec![],
                    request_id: None,
                },
            );
            other_interface
                .sink
                .as_ref()
                .unwrap()
                .send(InternalServicePayload::Response(response.clone()))
                .unwrap();
        } else {
            panic!("Expected to receive request");
        }

        // Receive response
        if let Some(received_response) = stream.next().await {
            // Test passed
        } else {
            panic!("Expected to receive response");
        }
    }

    // Disabled for WASM due to tokio runtime parking issues
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn test_in_memory_interface_next_connection() {
        // Create channels
        let (tx, rx) = mpsc::unbounded_channel::<InternalServicePayload>();

        let mut interface = InMemoryInterface {
            sink: Some(tx.clone()),
            stream: Some(rx),
        };

        // Get connection
        let (_sink, _stream) = interface.next_connection().await.unwrap();

        // Verify we can't get another connection (should be None)
        assert!(interface.next_connection().await.is_none());
    }
}

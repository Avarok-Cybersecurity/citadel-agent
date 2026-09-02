use citadel_internal_service_connector::io_interface::origin_policy::OriginPolicy;
use citadel_internal_service_connector::io_interface::{
    websockets::WebSocketInterface, IOInterface,
};
use citadel_internal_service_types::{
    InternalServicePayload, InternalServiceRequest, InternalServiceResponse,
};
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Starting Citadel WebSocket Test Server...");

    // Use 0.0.0.0 to allow connections from Docker containers
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8081".to_string());
    let addr: SocketAddr = bind_addr.parse()?;
    let mut interface = WebSocketInterface::new(addr, OriginPolicy::Any).await?;

    println!("✅ WebSocket server listening on {}", addr);
    println!("📡 Waiting for client connections...\n");

    while let Some((mut sink, mut stream)) = interface.next_connection().await {
        println!("👋 New client connected!");

        // Spawn a task to handle this client
        tokio::spawn(async move {
            let mut message_count = 0u64;

            while let Some(result) = stream.next().await {
                match result {
                    Ok(payload) => {
                        message_count += 1;
                        println!("📨 Received message #{}: {:?}", message_count, payload);

                        match payload {
                            InternalServicePayload::Request(request) => {
                                let response = handle_request(request, message_count).await;

                                if let Err(e) = sink.send(response).await {
                                    eprintln!("❌ Failed to send response: {}", e);
                                    break;
                                }

                                println!("✅ Sent response for message #{}", message_count);
                            }
                            _ => {
                                println!("⚠️  Received unexpected payload type");
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("❌ Error receiving message: {}", e);
                        break;
                    }
                }
            }

            println!("🔌 Client disconnected after {} messages", message_count);
        });
    }

    Ok(())
}

async fn handle_request(
    request: InternalServiceRequest,
    message_count: u64,
) -> InternalServicePayload {
    match request {
        InternalServiceRequest::Connect {
            request_id,
            username,
            ..
        } => {
            println!("🔐 Handling Connect request from user: {}", username);

            InternalServicePayload::Response(InternalServiceResponse::ConnectSuccess(
                citadel_internal_service_types::ConnectSuccess {
                    cid: 12345 + message_count, // Unique CID for each connection
                    request_id: Some(request_id),
                },
            ))
        }

        InternalServiceRequest::Message {
            request_id,
            message,
            cid,
            ..
        } => {
            let message_str = String::from_utf8_lossy(&message);
            println!(
                "💬 Handling Message request (CID: {}): {}",
                cid, message_str
            );

            InternalServicePayload::Response(InternalServiceResponse::MessageSendSuccess(
                citadel_internal_service_types::MessageSendSuccess {
                    cid,
                    peer_cid: None,
                    request_id: Some(request_id),
                },
            ))
        }

        InternalServiceRequest::Disconnect { request_id, cid } => {
            println!("🔌 Handling Disconnect request (CID: {})", cid);

            InternalServicePayload::Response(InternalServiceResponse::DisconnectNotification(
                citadel_internal_service_types::DisconnectNotification {
                    cid,
                    peer_cid: None,
                    request_id: Some(request_id),
                },
            ))
        }

        _ => {
            println!("⚠️  Unhandled request type");

            // Return a generic error response
            InternalServicePayload::Response(InternalServiceResponse::ConnectFailure(
                citadel_internal_service_types::ConnectFailure {
                    cid: 0,
                    message: "Unhandled request type".to_string(),
                    request_id: None,
                },
            ))
        }
    }
}

use citadel_internal_service_types::{
    InternalServicePayload, InternalServiceRequest, InternalServiceResponse, SecBuffer,
};
use std::time::Duration;
use uuid::Uuid;

fn main() {
    println!("=== WebSocket JSON Format Demo ===\n");

    // Example request that a frontend would send
    let request = InternalServicePayload::Request(InternalServiceRequest::Connect {
        request_id: Uuid::new_v4(),
        username: "frontend_user".to_string(),
        password: SecBuffer::from(b"my_password".to_vec()),
        connect_mode: Default::default(),
        udp_mode: Default::default(),
        keep_alive_timeout: Some(Duration::from_secs(30)),
        session_security_settings: Default::default(),
        server_password: None,
    });

    let request_json = serde_json::to_string_pretty(&request).unwrap();
    println!("Example JSON Request (Connect):");
    println!("{}\n", request_json);

    // Example response that the server would send back
    let response = InternalServicePayload::Response(InternalServiceResponse::ConnectSuccess(
        citadel_internal_service_types::ConnectSuccess {
            cid: 12345,
            request_id: Some(Uuid::new_v4()),
        },
    ));

    let response_json = serde_json::to_string_pretty(&response).unwrap();
    println!("Example JSON Response (ConnectSuccess):");
    println!("{}\n", response_json);

    // Example message request
    let message_request = InternalServicePayload::Request(InternalServiceRequest::Message {
        request_id: Uuid::new_v4(),
        message: b"Hello, Citadel!".to_vec(),
        cid: 12345,
        peer_cid: None, // None means send to server
        security_level: Default::default(),
    });

    let message_json = serde_json::to_string_pretty(&message_request).unwrap();
    println!("Example JSON Request (Message):");
    println!("{}\n", message_json);

    println!("=== Frontend Usage Instructions ===");
    println!("1. Connect to the WebSocket server");
    println!("2. Send JSON strings as Text messages (not Binary)");
    println!("3. Receive JSON strings as Text messages");
    println!("4. Parse the JSON to get the InternalServicePayload structure");
    println!("\nJavaScript Example:");
    println!("const ws = new WebSocket('ws://localhost:8081');");
    println!("ws.send(JSON.stringify(requestPayload));");
    println!("ws.onmessage = (event) => {{");
    println!("  const response = JSON.parse(event.data);");
    println!("  console.log('Received:', response);");
    println!("}};");
}

use citadel_internal_service::kernel::{CitadelWorkspaceService, RatchetType};
use citadel_internal_service_connector::io_interface::websockets::WebSocketInterface;
use citadel_sdk::prefabs::server::empty::EmptyKernel;
use citadel_sdk::prelude::*;
use std::net::{SocketAddr, TcpListener};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    env_logger::init();

    println!("🚀 Starting Citadel Internal Service with WebSocket support...");

    // 1. Start the Citadel server (for client registration)
    let server_listener = TcpListener::bind("127.0.0.1:0")?;
    let server_addr = server_listener.local_addr()?;
    println!("📡 Citadel server listening on {}", server_addr);

    let server_node = NodeBuilder::<RatchetType>::default()
        .with_node_type(NodeType::Server(server_addr))
        .with_insecure_skip_cert_verification()
        .with_underlying_protocol(ServerUnderlyingProtocol::from_std_tcp_listener(
            server_listener,
        )?)
        .build(EmptyKernel::default())?;

    tokio::task::spawn(async move {
        if let Err(e) = server_node.await {
            eprintln!("❌ Citadel server error: {}", e);
        }
    });

    // 2. Start the WebSocket internal service
    let websocket_addr: SocketAddr = "127.0.0.1:8080".parse()?;
    let websocket_interface = WebSocketInterface::new(websocket_addr).await?;

    println!("✅ WebSocket server listening on {}", websocket_addr);
    println!("📡 Clients should register with server: {}", server_addr);
    println!("📡 WebSocket clients connect to: {}", websocket_addr);
    println!("📡 Ready for client connections...\n");

    // Write server configuration to file for test consumption
    let config = format!(
        r#"{{
        "server_addr": "{}",
        "websocket_addr": "{}"
    }}"#,
        server_addr, websocket_addr
    );
    std::fs::write("server_config.json", config)?;

    // Create the CitadelWorkspaceService with WebSocket interface
    let service = CitadelWorkspaceService::<_, RatchetType>::new(websocket_interface);

    // Build and run the internal service node
    let internal_service_node = NodeBuilder::default()
        .with_node_type(NodeType::Peer)
        .with_backend(BackendType::InMemory)
        .with_insecure_skip_cert_verification()
        .build(service)?;

    // Run the internal service node
    internal_service_node.await?;

    Ok(())
}

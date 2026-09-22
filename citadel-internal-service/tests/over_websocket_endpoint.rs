//! Two agents, one workspace server reached only by WebSocket URL.
//!
//! The server is external -- a Cloudflare Durable Object under `wrangler dev`, or anything else
//! that serves a Citadel node at a `ws://`/`wss://` URL -- so this test is ignored unless it is
//! told where that is:
//!
//! ```text
//! CITADEL_WS_PROOF_ENDPOINT=ws://localhost:8797/acme \
//!   cargo test --test over_websocket_endpoint -- --ignored --nocapture
//! ```
//!
//! `CITADEL_WS_PROOF_INSECURE=1` skips certificate verification, for a local `wss://` edge with a
//! self-signed certificate (`wrangler dev --local-protocol https`). Without it the agents verify
//! the edge against the OS roots, as the released agent does.
//!
//! Everything else is the real thing: two `CitadelWorkspaceService` kernels, each in its own
//! Peer node, driven through the same request protocol the UI speaks. They register and log in
//! to the server by URL, register with each other through it, connect, and exchange a message
//! each way.

use citadel_internal_service_test_common as common;

use citadel_internal_service::kernel::CitadelWorkspaceService;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, MessageNotification, MessageSendSuccess,
};
use citadel_sdk::prelude::*;
use common::{
    connect_p2p, get_free_port, register_and_connect_to_server, register_p2p, spawn_services,
    InternalServicesFutures, RegisterAndConnectItems,
};
use std::error::Error;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

const ENDPOINT_VAR: &str = "CITADEL_WS_PROOF_ENDPOINT";
const INSECURE_VAR: &str = "CITADEL_WS_PROOF_INSECURE";

async fn spawn_agent(insecure: bool) -> Result<SocketAddr, Box<dyn Error>> {
    let bind: SocketAddr = format!("127.0.0.1:{}", get_free_port()).parse()?;
    let kernel = CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(bind).await?;
    let mut builder = NodeBuilder::default();
    let builder = builder
        .with_backend(BackendType::InMemory)
        .with_node_type(NodeType::Peer);
    if insecure {
        let _ = builder.with_insecure_skip_cert_verification();
    }
    let node = builder.build(kernel)?;
    let futures: Vec<InternalServicesFutures> = vec![Box::pin(async move {
        node.await
            .map(|_| ())
            .map_err(|err| Box::from(err) as Box<dyn Error>)
    })];
    spawn_services(futures);
    Ok(bind)
}

/// Frames the tenant worker's object has received on all its connections, from its stats page
/// (a plain GET on the same path). Only for a `ws://` endpoint: this is a measurement of where the
/// messages travelled, not part of what is proved, and a `wss://` edge needs a TLS client for it.
async fn object_frames(endpoint: &str) -> Option<u64> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let rest = endpoint.strip_prefix("ws://")?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let mut tcp = tokio::net::TcpStream::connect(authority).await.ok()?;
    let request = format!("GET /{path} HTTP/1.0\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    tcp.write_all(request.as_bytes()).await.ok()?;
    let mut response = String::new();
    tcp.read_to_string(&mut response).await.ok()?;
    Some(
        response
            .split("\"frames\":")
            .skip(1)
            .filter_map(|tail| {
                tail.split(|c: char| !c.is_ascii_digit())
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
            .sum(),
    )
}

async fn send_and_expect(
    from: (
        &UnboundedSender<InternalServiceRequest>,
        &mut UnboundedReceiver<InternalServiceResponse>,
        u64,
    ),
    to: (&mut UnboundedReceiver<InternalServiceResponse>, u64),
    text: &str,
) -> Result<(), Box<dyn Error>> {
    let (to_sender, from_sender, sender_cid) = from;
    let (from_receiver, receiver_cid) = to;
    let started = std::time::Instant::now();
    to_sender.send(InternalServiceRequest::Message {
        message: text.as_bytes().to_vec(),
        cid: sender_cid,
        peer_cid: Some(receiver_cid),
        security_level: Default::default(),
        request_id: Uuid::new_v4(),
    })?;
    match tokio::time::timeout(Duration::from_secs(30), from_sender.recv()).await? {
        Some(InternalServiceResponse::MessageSendSuccess(MessageSendSuccess { .. })) => {}
        other => return Err(format!("{sender_cid} could not send: {other:?}").into()),
    }
    match tokio::time::timeout(Duration::from_secs(30), from_receiver.recv()).await? {
        Some(InternalServiceResponse::MessageNotification(MessageNotification {
            message,
            cid,
            peer_cid,
            ..
        })) => {
            assert_eq!(cid, receiver_cid, "delivered to the wrong session");
            assert_eq!(peer_cid, sender_cid, "attributed to the wrong sender");
            assert_eq!(&*message, text.as_bytes(), "message altered in transit");
            println!(
                "DELIVERED {sender_cid} -> {receiver_cid}: {text:?} in {:?}",
                started.elapsed()
            );
            Ok(())
        }
        other => Err(format!("{receiver_cid} did not receive the message: {other:?}").into()),
    }
}

#[tokio::test]
#[ignore = "needs a Citadel server at CITADEL_WS_PROOF_ENDPOINT (e.g. wrangler dev)"]
async fn two_agents_exchange_a_p2p_message_through_a_websocket_server() -> Result<(), Box<dyn Error>>
{
    common::setup_log();
    let endpoint = std::env::var(ENDPOINT_VAR).map_err(|_| {
        format!("{ENDPOINT_VAR} must name the server, e.g. ws://localhost:8797/acme")
    })?;
    let insecure = std::env::var(INSECURE_VAR).as_deref() == Ok("1");

    let agent_a = spawn_agent(insecure).await?;
    let agent_b = spawn_agent(insecure).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let run = Uuid::new_v4().to_string();
    let items = [("a", agent_a), ("b", agent_b)]
        .into_iter()
        .map(|(who, agent)| RegisterAndConnectItems {
            internal_service_addr: agent,
            server_addr: endpoint.clone(),
            full_name: format!("Proof {who}"),
            username: format!("proof.{who}.{}", &run[..8]),
            password: format!("secret-{who}-{run}").into_bytes(),
            pre_shared_key: None::<PreSharedKey>,
        })
        .collect();
    let mut agents = register_and_connect_to_server(items).await?;
    println!("CONNECTED both agents to {endpoint}");

    let (a, b) = agents.split_at_mut(1);
    let (to_a, from_a, cid_a) = &mut a[0];
    let (to_b, from_b, cid_b) = &mut b[0];
    let settings = SessionSecuritySettingsBuilder::default().build()?;
    register_p2p(to_a, from_a, *cid_a, to_b, from_b, *cid_b, settings, None).await?;
    println!("P2P REGISTERED {cid_a} <-> {cid_b}");
    connect_p2p(to_a, from_a, *cid_a, to_b, from_b, *cid_b, settings, None).await?;
    println!("P2P CONNECTED {cid_a} <-> {cid_b}");

    let frames_before = object_frames(&endpoint).await;
    let sending = std::time::Instant::now();
    send_and_expect((to_a, from_a, *cid_a), (from_b, *cid_b), "hello from a").await?;
    send_and_expect(
        (to_b, from_b, *cid_b),
        (from_a, *cid_a),
        "hello back from b",
    )
    .await?;
    let frames_after = object_frames(&endpoint).await;
    // The same length of time with nothing sent, so keep-alives are not read as messages.
    tokio::time::sleep(sending.elapsed()).await;
    let frames_idle = object_frames(&endpoint).await;
    println!(
        "OBJECT FRAMES: before={frames_before:?} after two messages={frames_after:?} \
         after as long again idle={frames_idle:?}"
    );
    println!("PROOF PASS {endpoint}");
    Ok(())
}

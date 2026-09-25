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
const SECOND_ENDPOINT_VAR: &str = "CITADEL_WS_PROOF_SECOND_ENDPOINT";

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

/// One agent, two hosted workspaces behind one edge address, logged in to at the same time.
///
/// Both endpoints resolve to the edge's address; the SDK once keyed a client's in-flight
/// connections by that address alone, so the second registration or login collided with the
/// first (`ProvisionalConnectionExists`, "Localhost is already trying to connect to …").
/// `CITADEL_WS_PROOF_SECOND_ENDPOINT` names the other workspace, e.g. ws://localhost:8797/globex.
#[tokio::test]
#[ignore = "needs two Citadel servers behind one address (e.g. wrangler dev, two tenants)"]
async fn one_agent_logs_in_to_two_workspaces_behind_one_edge_at_once() -> Result<(), Box<dyn Error>>
{
    common::setup_log();
    let first = std::env::var(ENDPOINT_VAR)
        .map_err(|_| format!("{ENDPOINT_VAR} must name the first workspace"))?;
    let second = std::env::var(SECOND_ENDPOINT_VAR)
        .map_err(|_| format!("{SECOND_ENDPOINT_VAR} must name the second workspace"))?;
    let insecure = std::env::var(INSECURE_VAR).as_deref() == Ok("1");
    let agent = spawn_agent(insecure).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let run = Uuid::new_v4().to_string();
    let item = |endpoint: &str, who: &str| RegisterAndConnectItems {
        internal_service_addr: agent,
        server_addr: endpoint.to_string(),
        full_name: format!("Proof {who}"),
        username: format!("proof.{who}.{}", &run[..8]),
        password: format!("secret-{who}-{run}").into_bytes(),
        pre_shared_key: None::<PreSharedKey>,
    };
    let (a, b) = tokio::join!(
        register_and_connect_to_server(vec![item(&first, "first")]),
        register_and_connect_to_server(vec![item(&second, "second")]),
    );
    let (a, b) = (a?, b?);
    println!(
        "CONNECTED one agent to {first} (cid {}) and {second} (cid {}) concurrently",
        a[0].2, b[0].2
    );
    assert_ne!(a[0].2, b[0].2);
    println!("PROOF PASS two workspaces, one edge address, one agent");
    Ok(())
}

async fn sever_if(at: &str) -> Result<(), Box<dyn Error>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    if std::env::var("CITADEL_WS_PROOF_SEVER_AT").as_deref() != Ok(at) {
        return Ok(());
    }
    let sever = std::env::var("CITADEL_WS_PROOF_SEVER")?;
    let mut tcp = tokio::net::TcpStream::connect(&sever).await?;
    tcp.write_all(
        format!(
            "GET {} HTTP/1.0\r\n\r\n",
            std::env::var("CITADEL_WS_PROOF_SEVER_PATH").unwrap_or_else(|_| "/".into())
        )
        .as_bytes(),
    )
    .await?;
    let mut out = String::new();
    let _ = tcp.read_to_string(&mut out).await;
    println!("SEVERED at {at}: {out:?}");
    Ok(())
}

async fn drain_until<T>(
    rx: &mut UnboundedReceiver<InternalServiceResponse>,
    who: &str,
    wait: Duration,
    mut pick: impl FnMut(&InternalServiceResponse) -> Option<T>,
) -> Result<T, Box<dyn Error>> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let next = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .map_err(|_| format!("{who}: timed out"))?
            .ok_or_else(|| format!("{who}: stream ended"))?;
        println!("[{who}] {next:?}");
        if let Some(found) = pick(&next) {
            return Ok(found);
        }
    }
}

#[tokio::test]
#[ignore = "needs a Citadel server at CITADEL_WS_PROOF_ENDPOINT (e.g. wrangler dev)"]
async fn two_agents_round_trip_a_group_through_a_websocket_server() -> Result<(), Box<dyn Error>> {
    use citadel_internal_service_types::{GroupCreateSuccess, GroupMessageNotification};
    common::setup_log();
    let endpoint =
        std::env::var(ENDPOINT_VAR).map_err(|_| format!("{ENDPOINT_VAR} must name the server"))?;
    let insecure = std::env::var(INSECURE_VAR).as_deref() == Ok("1");
    let agent_a = spawn_agent(insecure).await?;
    let agent_b = if std::env::var("CITADEL_WS_PROOF_ONE_AGENT").as_deref() == Ok("1") {
        agent_a
    } else {
        spawn_agent(insecure).await?
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    let run = Uuid::new_v4().to_string();
    let items = [("a", agent_a), ("b", agent_b)]
        .into_iter()
        .map(|(who, agent)| RegisterAndConnectItems {
            internal_service_addr: agent,
            server_addr: endpoint.clone(),
            full_name: format!("Group {who}"),
            username: format!("group.{who}.{}", &run[..8]),
            password: format!("secret-{who}-{run}").into_bytes(),
            pre_shared_key: None::<PreSharedKey>,
        })
        .collect();
    let mut agents = register_and_connect_to_server(items).await?;
    let (a, b) = agents.split_at_mut(1);
    let (to_a, from_a, cid_a) = &mut a[0];
    let (to_b, from_b, cid_b) = &mut b[0];
    let (cid_a, cid_b) = (*cid_a, *cid_b);
    let settings = SessionSecuritySettingsBuilder::default().build()?;
    register_p2p(to_a, from_a, cid_a, to_b, from_b, cid_b, settings, None).await?;
    println!("P2P REGISTERED {cid_a} <-> {cid_b}");

    for round in 0..2 {
        to_a.send(InternalServiceRequest::GroupCreate {
            cid: cid_a,
            request_id: Uuid::new_v4(),
            initial_users_to_invite: Some(vec![cid_b.into()]),
        })?;
        let key = drain_until(from_a, "a", Duration::from_secs(30), |r| match r {
            InternalServiceResponse::GroupCreateSuccess(GroupCreateSuccess {
                group_key, ..
            }) => Some(*group_key),
            _ => None,
        })
        .await?;
        if round == 0 {
            sever_if("after_create").await?;
        }
        drain_until(from_b, "b", Duration::from_secs(60), |r| {
            matches!(r, InternalServiceResponse::GroupInviteNotification(..)).then_some(())
        })
        .await?;
        if round == 0 && std::env::var("CITADEL_WS_PROOF_SEVER_AT").as_deref() == Ok("after_create")
        {
            for (rx, who) in [(&mut *from_a, "a"), (&mut *from_b, "b")] {
                drain_until(rx, who, Duration::from_secs(60), |r| {
                    matches!(r, InternalServiceResponse::ServerReconnected(..)).then_some(())
                })
                .await?;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        to_b.send(InternalServiceRequest::GroupRespondRequest {
            cid: cid_b,
            peer_cid: cid_a,
            group_key: key,
            response: true,
            request_id: Uuid::new_v4(),
            invitation: true,
        })?;
        drain_until(from_b, "b", Duration::from_secs(30), |r| {
            matches!(r, InternalServiceResponse::GroupRespondRequestSuccess(..)).then_some(())
        })
        .await?;
        if round == 0 {
            sever_if("after_accept").await?;
        }
        for (to, from_rx, sender, receiver, rx_name) in [
            (&*to_a, &mut *from_b, cid_a, cid_b, "b"),
            (&*to_b, &mut *from_a, cid_b, cid_a, "a"),
        ] {
            let text = format!("round {round} from {sender}");
            let mut delivered = false;
            for _ in 0..40 {
                to.send(InternalServiceRequest::GroupMessage {
                    cid: sender,
                    message: text.clone().into_bytes(),
                    group_key: key,
                    request_id: Uuid::new_v4(),
                })?;
                let got = drain_until(from_rx, rx_name, Duration::from_secs(1), |r| match r {
                    InternalServiceResponse::GroupMessageNotification(
                        GroupMessageNotification { cid, group_key, .. },
                    ) if *cid == receiver && *group_key == key => Some(()),
                    _ => None,
                })
                .await;
                if got.is_ok() {
                    delivered = true;
                    break;
                }
            }
            assert!(
                delivered,
                "round {round}: {receiver} never received {sender}'s group message"
            );
            println!("GROUP DELIVERED round {round} {sender} -> {receiver}");
        }
        if round == 0 {
            let severs: u32 = std::env::var("CITADEL_WS_PROOF_SEVERS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            for sever_round in 0..severs {
                if let (Ok(sever), Ok("after_round")) = (
                    std::env::var("CITADEL_WS_PROOF_SEVER"),
                    std::env::var("CITADEL_WS_PROOF_SEVER_AT").as_deref(),
                ) {
                    let sever_started = std::time::Instant::now();
                    println!("SEVER ROUND {sever_round}");
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut tcp = tokio::net::TcpStream::connect(&sever).await?;
                    tcp.write_all(
                        format!(
                            "GET {} HTTP/1.0\r\n\r\n",
                            std::env::var("CITADEL_WS_PROOF_SEVER_PATH")
                                .unwrap_or_else(|_| "/".into())
                        )
                        .as_bytes(),
                    )
                    .await?;
                    let mut out = String::new();
                    let _ = tcp.read_to_string(&mut out).await;
                    println!("SEVERED the server links: {out:?}");
                    for (to, from_rx, sender, receiver, rx_name) in [
                        (&*to_a, &mut *from_b, cid_a, cid_b, "b"),
                        (&*to_b, &mut *from_a, cid_b, cid_a, "a"),
                    ] {
                        let mut delivered = false;
                        for n in 0..240 {
                            to.send(InternalServiceRequest::GroupMessage {
                                cid: sender,
                                message: format!("after sever {n}").into_bytes(),
                                group_key: key,
                                request_id: Uuid::new_v4(),
                            })?;
                            if drain_until(from_rx, rx_name, Duration::from_secs(1), |r| match r {
                                InternalServiceResponse::GroupMessageNotification(
                                    GroupMessageNotification { cid, group_key, .. },
                                ) if *cid == receiver && *group_key == key => Some(()),
                                _ => None,
                            })
                            .await
                            .is_ok()
                            {
                                delivered = true;
                                break;
                            }
                        }
                        assert!(delivered, "after sever {sever_round}: {receiver} never received {sender}'s group message");
                        println!("GROUP DELIVERED after sever {sever_round} {sender} -> {receiver} at {:?}", sever_started.elapsed());
                    }
                }
            }
        }
    }
    println!("PROOF PASS group round trip twice through {endpoint}");
    Ok(())
}

async fn chaos_control(path: &str) -> Result<(), Box<dyn Error>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let control = std::env::var("CITADEL_WS_PROOF_CHAOS")?;
    let mut tcp = tokio::net::TcpStream::connect(&control).await?;
    tcp.write_all(format!("GET {path} HTTP/1.0\r\n\r\n").as_bytes())
        .await?;
    let mut out = String::new();
    let _ = tcp.read_to_string(&mut out).await;
    println!("CHAOS {path}: {:?}", out.lines().last());
    Ok(())
}

fn drain_now(rx: &mut UnboundedReceiver<InternalServiceResponse>, who: &str) {
    while let Ok(r) = rx.try_recv() {
        println!("[{who}] (drained) {r:?}");
    }
}

/// One round: create, invite, accept, deliver both ways. `Err` names the stage that failed.
async fn group_round(
    to_a: &UnboundedSender<InternalServiceRequest>,
    from_a: &mut UnboundedReceiver<InternalServiceResponse>,
    cid_a: u64,
    to_b: &UnboundedSender<InternalServiceRequest>,
    from_b: &mut UnboundedReceiver<InternalServiceResponse>,
    cid_b: u64,
    sever_at: Option<(u8, &str)>,
) -> Result<(), String> {
    use citadel_internal_service_types::{GroupCreateSuccess, GroupMessageNotification};
    let sever = |stage: u8| async move {
        if let Some((at, path)) = sever_at {
            if at == stage {
                let _ = chaos_control(path).await;
            }
        }
    };
    sever(0).await;
    to_a.send(InternalServiceRequest::GroupCreate {
        cid: cid_a,
        request_id: Uuid::new_v4(),
        initial_users_to_invite: Some(vec![cid_b.into()]),
    })
    .map_err(|e| e.to_string())?;
    let key = drain_until(from_a, "a", Duration::from_secs(40), |r| match r {
        InternalServiceResponse::GroupCreateSuccess(GroupCreateSuccess { group_key, .. }) => {
            Some(Ok(*group_key))
        }
        InternalServiceResponse::GroupCreateFailure(f) => Some(Err(format!("{f:?}"))),
        _ => None,
    })
    .await
    .map_err(|e| format!("create: {e}"))?
    .map_err(|e| format!("create failed: {e}"))?;
    sever(1).await;
    drain_until(from_b, "b", Duration::from_secs(40), |r| match r {
        InternalServiceResponse::GroupInviteNotification(n) if n.group_key == key => Some(()),
        _ => None,
    })
    .await
    .map_err(|e| format!("invite: {e}"))?;
    to_b.send(InternalServiceRequest::GroupRespondRequest {
        cid: cid_b,
        peer_cid: cid_a,
        group_key: key,
        response: true,
        request_id: Uuid::new_v4(),
        invitation: true,
    })
    .map_err(|e| e.to_string())?;
    sever(2).await;
    let accepted = drain_until(from_b, "b", Duration::from_secs(45), |r| match r {
        InternalServiceResponse::GroupRespondRequestSuccess(s) if s.group_key == key => Some(true),
        InternalServiceResponse::GroupRespondRequestFailure(_) => Some(false),
        _ => None,
    })
    .await
    .map_err(|e| format!("accept: {e}"))?;
    println!("ACCEPT answered: {accepted}");
    sever(3).await;
    for (to, rx, sender, receiver, name) in [
        (to_a, &mut *from_b, cid_a, cid_b, "b"),
        (to_b, &mut *from_a, cid_b, cid_a, "a"),
    ] {
        let mut delivered = false;
        for n in 0..60 {
            let _ = to.send(InternalServiceRequest::GroupMessage {
                cid: sender,
                message: format!("chaos {n}").into_bytes(),
                group_key: key,
                request_id: Uuid::new_v4(),
            });
            if drain_until(rx, name, Duration::from_secs(1), |r| match r {
                InternalServiceResponse::GroupMessageNotification(GroupMessageNotification {
                    cid,
                    group_key,
                    ..
                }) if *cid == receiver && *group_key == key => Some(()),
                _ => None,
            })
            .await
            .is_ok()
            {
                delivered = true;
                break;
            }
        }
        if !delivered {
            return Err(format!("deliver {sender} -> {receiver}"));
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "needs CITADEL_WS_PROOF_ENDPOINT behind chaosproxy (CITADEL_WS_PROOF_CHAOS)"]
async fn group_rounds_survive_link_chaos() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let endpoint = std::env::var(ENDPOINT_VAR)?;
    let rounds: u32 = std::env::var("CITADEL_WS_PROOF_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let agent = spawn_agent(false).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let run = Uuid::new_v4().to_string();
    let items = ["a", "b"]
        .into_iter()
        .map(|who| RegisterAndConnectItems {
            internal_service_addr: agent,
            server_addr: endpoint.clone(),
            full_name: format!("Chaos {who}"),
            username: format!("chaos.{who}.{}", &run[..8]),
            password: format!("secret-{who}-{run}").into_bytes(),
            pre_shared_key: None::<PreSharedKey>,
        })
        .collect();
    let mut agents = register_and_connect_to_server(items).await?;
    let (a, b) = agents.split_at_mut(1);
    let (to_a, from_a, cid_a) = &mut a[0];
    let (to_b, from_b, cid_b) = &mut b[0];
    let (cid_a, cid_b) = (*cid_a, *cid_b);
    let settings = SessionSecuritySettingsBuilder::default().build()?;
    register_p2p(to_a, from_a, cid_a, to_b, from_b, cid_b, settings, None).await?;
    let mut tally = Vec::new();
    for round in 0..rounds {
        let stage = (round % 4) as u8;
        let path = if round % 3 == 0 {
            "/sever"
        } else {
            "/sever-one"
        };
        let outcome = group_round(
            to_a,
            from_a,
            cid_a,
            to_b,
            from_b,
            cid_b,
            Some((stage, path)),
        )
        .await;
        println!("ROUND {round} sever {path} at stage {stage}: {outcome:?}");
        tally.push(outcome.is_ok());
        tokio::time::sleep(Duration::from_secs(12)).await;
        drain_now(from_a, "a");
        drain_now(from_b, "b");
    }
    let clean = group_round(to_a, from_a, cid_a, to_b, from_b, cid_b, None).await;
    println!("CHAOS TALLY {tally:?}; clean round after the chaos: {clean:?}");
    clean.map_err(|e| format!("a clean group round after the chaos failed: {e}").into())
}

async fn control_get(addr: &str, path: &str) -> Result<(), Box<dyn Error>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut tcp = tokio::net::TcpStream::connect(addr).await?;
    tcp.write_all(format!("GET {path} HTTP/1.0\r\n\r\n").as_bytes())
        .await?;
    let mut out = String::new();
    let _ = tcp.read_to_string(&mut out).await;
    Ok(())
}

/// The owner on a slow link, the member on a fast one, both cut at once: the member's
/// restore KeyPackage can reach the owner's new session before its RestoreOwnership.
#[tokio::test]
#[ignore = "needs two proxies in front of one tenant (CITADEL_WS_PROOF_SLOW/FAST + controls)"]
async fn a_member_restore_racing_the_owner_restore_does_not_kill_the_owner(
) -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let slow = std::env::var("CITADEL_WS_PROOF_SLOW")?;
    let fast = std::env::var("CITADEL_WS_PROOF_FAST")?;
    let slow_ctl = std::env::var("CITADEL_WS_PROOF_SLOW_CTL")?;
    let fast_ctl = std::env::var("CITADEL_WS_PROOF_FAST_CTL")?;
    let cycles: u32 = std::env::var("CITADEL_WS_PROOF_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let agent = spawn_agent(false).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let run = Uuid::new_v4().to_string();
    let items = [("a", &slow), ("b", &fast)]
        .into_iter()
        .map(|(who, endpoint)| RegisterAndConnectItems {
            internal_service_addr: agent,
            server_addr: endpoint.clone(),
            full_name: format!("Race {who}"),
            username: format!("race.{who}.{}", &run[..8]),
            password: format!("secret-{who}-{run}").into_bytes(),
            pre_shared_key: None::<PreSharedKey>,
        })
        .collect();
    let mut agents = register_and_connect_to_server(items).await?;
    let (a, b) = agents.split_at_mut(1);
    let (to_a, from_a, cid_a) = &mut a[0];
    let (to_b, from_b, cid_b) = &mut b[0];
    let (cid_a, cid_b) = (*cid_a, *cid_b);
    let settings = SessionSecuritySettingsBuilder::default().build()?;
    register_p2p(to_a, from_a, cid_a, to_b, from_b, cid_b, settings, None).await?;
    group_round(to_a, from_a, cid_a, to_b, from_b, cid_b, None)
        .await
        .map_err(|e| format!("control round before any cut: {e}"))?;
    let mut unrequested = 0usize;
    for cycle in 0..cycles {
        control_get(&slow_ctl, "/sever").await?;
        control_get(&fast_ctl, "/sever").await?;
        println!("CUT both links (cycle {cycle})");
        // Both sessions back first; any drop after that was not caused by the cut.
        let mut back = [false, false];
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        let mut quiet_until: Option<tokio::time::Instant> = None;
        loop {
            let now = tokio::time::Instant::now();
            if quiet_until.is_some_and(|q| now >= q) || now >= deadline {
                break;
            }
            for (i, (rx, who)) in [(&mut *from_a, "a"), (&mut *from_b, "b")]
                .into_iter()
                .enumerate()
            {
                while let Ok(r) = rx.try_recv() {
                    match &r {
                        InternalServiceResponse::ServerReconnected(..) => back[i] = true,
                        InternalServiceResponse::ServerConnectionLost(..)
                            if quiet_until.is_some() =>
                        {
                            unrequested += 1;
                            println!("UNREQUESTED DROP of {who} in cycle {cycle}");
                        }
                        _ => {}
                    }
                    println!("[{who}] (cycle {cycle}) {r:?}");
                }
            }
            if quiet_until.is_none() && back == [true, true] {
                quiet_until = Some(tokio::time::Instant::now() + Duration::from_secs(15));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            back == [true, true],
            "cycle {cycle}: a session never came back: {back:?}"
        );
    }
    println!("UNREQUESTED DROPS {unrequested} in {cycles} cycles");
    group_round(to_a, from_a, cid_a, to_b, from_b, cid_b, None)
        .await
        .map_err(|e| format!("a new group after the cuts: {e}"))?;
    assert_eq!(
        unrequested, 0,
        "a session dropped after both had reconnected, and nobody cut it"
    );
    Ok(())
}

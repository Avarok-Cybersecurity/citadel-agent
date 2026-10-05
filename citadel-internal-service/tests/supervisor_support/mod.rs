//! Helpers for `connection_supervisor.rs`: a supervised agent, a hosted peer, and the two
//! signed in and registered to each other over a server the first reaches through a `Proxy`.

use crate::reconnect::Proxy;
use crate::support::{expect, open, register, temp_store, username, Stream};
use citadel_internal_service::kernel::CitadelWorkspaceService;
use citadel_internal_service::{SupervisorPolicy, SERVER_RECONNECT};
use citadel_internal_service_connector::connector::WrappedSink;
use citadel_internal_service_connector::io_interface::tcp::TcpIOInterface;
use citadel_internal_service_test_common as common;
use citadel_internal_service_test_common::agent_ilm::declare_agent_ilm;
use citadel_internal_service_types::{
    AgentCapabilities, InternalServiceRequest, InternalServiceResponse, PeerRegisterSuccess,
};
use citadel_sdk::prelude::*;
use std::error::Error;
use std::net::SocketAddr;
use std::path::Path;
use tokio::task::JoinHandle;
use uuid::Uuid;

pub struct Window {
    pub sink: WrappedSink<TcpIOInterface>,
    pub stream: Stream,
    pub cid: u64,
}

pub struct Pair {
    pub proxy: Proxy,
    pub a_caps: AgentCapabilities,
    pub b_caps: AgentCapabilities,
    pub a: Window,
    pub b: Window,
    _nodes: Vec<JoinHandle<()>>,
}

/// An agent storing under `store`, supervising by `policy` when given one.
pub async fn spawn_agent(
    store: &Path,
    policy: Option<SupervisorPolicy>,
) -> Result<(SocketAddr, JoinHandle<()>), Box<dyn Error>> {
    let bind: SocketAddr = format!("127.0.0.1:{}", common::get_free_port()).parse()?;
    let mut kernel =
        CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(bind, SERVER_RECONNECT).await?;
    if let Some(policy) = policy {
        kernel = kernel.with_supervisor(policy);
    }
    let node = common::test_stun_servers()
        .apply(&mut NodeBuilder::<StackedRatchet>::default())
        .with_backend(BackendType::Filesystem(
            store.to_string_lossy().into_owned(),
        ))
        .with_node_type(NodeType::Peer)
        .with_insecure_skip_cert_verification()
        .build(kernel)?;
    Ok((
        bind,
        tokio::spawn(async move {
            let _ = node.await;
        }),
    ))
}

/// Declares the window hosted, and returns what the agent says it does.
pub async fn declare(window: &mut (WrappedSink<TcpIOInterface>, Stream)) -> AgentCapabilities {
    common::send(&mut window.0, declare_agent_ilm())
        .await
        .expect("the agent is up");
    expect(&mut window.1, |r| match r {
        InternalServiceResponse::AgentCapabilities(c) => Some(c),
        _ => None,
    })
    .await
    .expect("the agent answers a declaration")
}

/// `a` (through the proxy, supervised by `a_policy`) and `b` (direct, supervising by
/// `b_policy`), each hosted and signed in, and registered to each other.
pub async fn pair(
    a_policy: Option<SupervisorPolicy>,
    b_policy: Option<SupervisorPolicy>,
) -> Result<Pair, Box<dyn Error>> {
    common::setup_log();
    let (server, server_addr) = common::server_info_skip_cert_verification::<StackedRatchet>();
    tokio::spawn(server);
    let proxy = Proxy::start(server_addr).await?;
    let (a_agent, a_node) = spawn_agent(&temp_store(), a_policy).await?;
    let (b_agent, b_node) = spawn_agent(&temp_store(), b_policy).await?;
    let mut a = open(a_agent).await?;
    let mut b = open(b_agent).await?;
    let a_caps = declare(&mut a).await;
    let b_caps = declare(&mut b).await;
    let cid_a = register(&mut a.0, &mut a.1, &proxy.addr.to_string(), &username()).await??;
    let cid_b = register(&mut b.0, &mut b.1, &server_addr.to_string(), &username()).await??;
    peer_register(&mut a, cid_a, &mut b, cid_b).await?;
    Ok(Pair {
        proxy,
        a_caps,
        b_caps,
        a: Window {
            sink: a.0,
            stream: a.1,
            cid: cid_a,
        },
        b: Window {
            sink: b.0,
            stream: b.1,
            cid: cid_b,
        },
        _nodes: vec![a_node, b_node],
    })
}

async fn peer_register(
    a: &mut (WrappedSink<TcpIOInterface>, Stream),
    cid_a: u64,
    b: &mut (WrappedSink<TcpIOInterface>, Stream),
    cid_b: u64,
) -> Result<(), Box<dyn Error>> {
    let request = |cid, peer_cid| InternalServiceRequest::PeerRegister {
        request_id: Uuid::new_v4(),
        cid,
        peer_cid,
        session_security_settings: Default::default(),
        connect_after_register: false,
        peer_session_password: None,
    };
    common::send(&mut a.0, request(cid_a, cid_b)).await?;
    expect(&mut b.1, |r| {
        matches!(r, InternalServiceResponse::PeerRegisterNotification(_)).then_some(())
    })
    .await?;
    common::send(&mut b.0, request(cid_b, cid_a)).await?;
    for stream in [&mut a.1, &mut b.1] {
        expect(stream, |r| {
            matches!(
                r,
                InternalServiceResponse::PeerRegisterSuccess(PeerRegisterSuccess { .. })
            )
            .then_some(())
        })
        .await?;
    }
    Ok(())
}

pub fn peer_connect(cid: u64, peer_cid: u64) -> InternalServiceRequest {
    InternalServiceRequest::PeerConnect {
        request_id: Uuid::new_v4(),
        cid,
        peer_cid,
        udp_mode: UdpMode::Disabled,
        session_security_settings: Default::default(),
        peer_session_password: None,
        turn: None,
    }
}

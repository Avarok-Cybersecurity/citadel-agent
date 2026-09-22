//! Helpers for `account_server_host.rs`: a server, restartable agents, and the requests.

use citadel_internal_service_test_common as common;

use citadel_internal_service::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::connector::{
    InternalServiceConnector, WrappedSink, WrappedStream,
};
use citadel_internal_service_connector::io_interface::tcp::TcpIOInterface;
use citadel_internal_service_types::{
    AccountInformation, Accounts, GetSessionsResponse, InternalServiceRequest,
    InternalServiceResponse, RegisterFailure, SessionInformation,
};
use citadel_sdk::prefabs::server::empty::EmptyKernel;
use citadel_sdk::prelude::*;
use futures::StreamExt;
use std::error::Error;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;
use tokio::task::JoinHandle;
use uuid::Uuid;

pub type Stream = WrappedStream<TcpIOInterface>;

pub const PASSWORD: &str = "server-host-password";
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// A workspace server on the first address `localhost` resolves to, so registering to
/// `localhost:<port>` dials the address the server is actually on.
pub async fn spawn_server() -> Result<u16, Box<dyn Error>> {
    let first = tokio::net::lookup_host("localhost:0")
        .await?
        .next()
        .ok_or("localhost resolves to nothing")?;
    let listener = std::net::TcpListener::bind(first)?;
    let (server, addr) = common::server_test_node_on_listener(
        EmptyKernel::<StackedRatchet>::default(),
        listener,
        |_| {},
    );
    tokio::spawn(server);
    Ok(addr.port())
}

/// An agent storing its accounts under `store`. Aborting the handle stops it.
pub async fn spawn_agent(store: &Path) -> Result<(SocketAddr, JoinHandle<()>), Box<dyn Error>> {
    let bind: SocketAddr = format!("127.0.0.1:{}", common::get_free_port()).parse()?;
    let kernel = CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(bind).await?;
    let mut builder = NodeBuilder::<StackedRatchet>::default();
    let node = builder
        .with_backend(BackendType::Filesystem(
            store.to_string_lossy().into_owned(),
        ))
        .with_node_type(NodeType::Peer)
        .build(kernel)?;
    let handle = tokio::spawn(async move {
        let _ = node.await;
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok((bind, handle))
}

pub fn temp_store() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("citadel-server-host-{}", Uuid::new_v4()))
}

pub async fn open(
    agent: SocketAddr,
) -> Result<(WrappedSink<TcpIOInterface>, Stream), Box<dyn Error>> {
    Ok(InternalServiceConnector::connect(agent).await?.split())
}

/// The next response `pick` accepts, skipping notifications that are not it.
pub async fn expect<T>(
    stream: &mut Stream,
    mut pick: impl FnMut(InternalServiceResponse) -> Option<T>,
) -> Result<T, Box<dyn Error>> {
    loop {
        let response = tokio::time::timeout(TIMEOUT, stream.next())
            .await?
            .ok_or("agent closed the connection")?;
        if let Some(found) = pick(response) {
            return Ok(found);
        }
    }
}

pub async fn register(
    sink: &mut WrappedSink<TcpIOInterface>,
    stream: &mut Stream,
    server_addr: &str,
    username: &str,
) -> Result<Result<u64, String>, Box<dyn Error>> {
    common::send(
        sink,
        InternalServiceRequest::Register {
            request_id: Uuid::new_v4(),
            server_addr: server_addr.to_string(),
            full_name: "Server Host".to_string(),
            username: username.to_string(),
            proposed_password: PASSWORD.as_bytes().to_vec().into(),
            connect_after_register: true,
            session_security_settings: Default::default(),
            server_password: None,
        },
    )
    .await?;
    expect(stream, |response| match response {
        InternalServiceResponse::ConnectSuccess(success) => Some(Ok(success.cid)),
        InternalServiceResponse::RegisterFailure(RegisterFailure { message, .. }) => {
            Some(Err(message))
        }
        InternalServiceResponse::ConnectFailure(failure) => Some(Err(failure.message)),
        _ => None,
    })
    .await
}

pub async fn account(
    sink: &mut WrappedSink<TcpIOInterface>,
    stream: &mut Stream,
    cid: Option<u64>,
) -> Result<Vec<(u64, AccountInformation)>, Box<dyn Error>> {
    let request_id = Uuid::new_v4();
    common::send(
        sink,
        InternalServiceRequest::GetAccountInformation { request_id, cid },
    )
    .await?;
    expect(stream, |response| match response {
        InternalServiceResponse::GetAccountInformationResponse(Accounts {
            accounts,
            request_id: Some(id),
            ..
        }) if id == request_id => Some(accounts.into_iter().collect()),
        _ => None,
    })
    .await
}

pub async fn session(
    sink: &mut WrappedSink<TcpIOInterface>,
    stream: &mut Stream,
    cid: u64,
) -> Result<SessionInformation, Box<dyn Error>> {
    let request_id = Uuid::new_v4();
    common::send(sink, InternalServiceRequest::GetSessions { request_id }).await?;
    let sessions = expect(stream, |response| match response {
        InternalServiceResponse::GetSessionsResponse(GetSessionsResponse {
            sessions,
            request_id: Some(id),
            ..
        }) if id == request_id => Some(sessions),
        _ => None,
    })
    .await?;
    Ok(sessions
        .into_iter()
        .find(|session| session.cid == cid)
        .ok_or(format!("no session for {cid}"))?)
}

pub fn username() -> String {
    format!("host.{}", &Uuid::new_v4().to_string()[..8])
}

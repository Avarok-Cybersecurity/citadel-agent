//! Helpers for the identity tests: an agent holding an identity, and workspace servers that
//! answer the `ServerIdentityChallenge` with a key the test can swap.
//!
//! The server and the codec speak the workspace protocol's JSON for the two messages
//! (`{"Request":{"ServerIdentityChallenge":{"nonce":[…]}}}` and
//! `{"Response":{"ServerIdentity":{"domain","pk","sig"}}}`), which the shipped codec gets from
//! `citadel-workspace-types` in the parent repository; this repository does not depend on it.

use citadel_identity::{canonical_domain, fingerprint, ServerKey};
use citadel_internal_service::kernel::identity::{
    IdentityConfig, MemoryIdentityStore, MonotonicClock, SignLimits,
};
use citadel_internal_service::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::connector::{InternalServiceConnector, WrappedSink};
use citadel_internal_service_connector::io_interface::tcp::TcpIOInterface;
use citadel_internal_service_test_common as common;
use citadel_sdk::prefabs::server::client_connect_listener::ClientConnectListenerKernel;
use citadel_sdk::prelude::*;
use futures::StreamExt;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

pub use crate::support::{expect, Stream};

pub const UNLOCK: &str = "a long unlock passphrase";

mod codec;
mod requests;
pub use codec::{JsonCodec, NoFetch};
pub use requests::*;

/// A server answering every challenge with the key `seed` currently holds.
pub struct ChallengeServer {
    /// The name it is dialled and signs under.
    pub addr: String,
    /// Where it actually listens.
    pub bound: SocketAddr,
    pub seed: Arc<Mutex<[u8; 32]>>,
    pub task: JoinHandle<()>,
}

impl ChallengeServer {
    pub fn fp(&self) -> String {
        fingerprint(ServerKey::from_seed(&self.seed.lock()).public_key())
    }
}

/// On a fresh port of the first address `localhost` resolves to. With `named_port`, the server
/// is reached through a front on that port (`spawn_front`) and signs under its name.
pub async fn spawn_challenge_server(
    named_port: Option<u16>,
    seed: Arc<Mutex<[u8; 32]>>,
) -> Result<ChallengeServer, Box<dyn Error>> {
    let first = tokio::net::lookup_host("localhost:0")
        .await?
        .next()
        .ok_or("localhost resolves to nothing")?;
    let listener = std::net::TcpListener::bind(first)?;
    let port = named_port.unwrap_or(listener.local_addr()?.port());
    let addr = format!("localhost:{port}");
    let domain = canonical_domain(&addr)?;
    let answering = seed.clone();
    let kernel = ClientConnectListenerKernel::new(
        move |conn: CitadelClientServerConnection<StackedRatchet>| {
            let (seed, domain) = (answering.clone(), domain.clone());
            async move {
                let cid = conn.cid;
                let (mut sink, mut stream) = conn.split();
                while let Some(frame) = stream.next().await {
                    let Ok(value) = serde_json::from_slice::<Value>(frame.as_ref()) else {
                        continue;
                    };
                    let Some(nonce) =
                        value["Request"]["ServerIdentityChallenge"]["nonce"].as_array()
                    else {
                        continue;
                    };
                    let nonce: Vec<u8> = nonce
                        .iter()
                        .filter_map(|n| n.as_u64())
                        .map(|n| n as u8)
                        .collect();
                    let nonce: [u8; 32] = nonce.try_into().expect("a 32-byte nonce");
                    let key = ServerKey::from_seed(&seed.lock());
                    let answer = json!({"Response": {"ServerIdentity": {
                        "domain": domain.as_str(),
                        "pk": key.public_key().as_bytes().to_vec(),
                        "sig": key.sign_challenge(&domain, cid, &nonce),
                    }}});
                    sink.send(serde_json::to_vec(&answer).unwrap()).await?;
                }
                Ok(())
            }
        },
    );
    let bound = listener.local_addr()?;
    let (server, _) = common::server_test_node_on_listener(
        Box::new(kernel) as Box<dyn NetKernel<StackedRatchet>>,
        listener,
        |_| {},
    );
    let task = tokio::spawn(async move {
        let _ = server.await;
    });
    Ok(ChallengeServer {
        addr,
        bound,
        seed,
        task,
    })
}

/// A TCP front on a fixed port forwarding to whichever server `target` names, so a server can
/// be "re-created" at the same address with a fresh database.
pub async fn spawn_front(
    target: SocketAddr,
) -> Result<(u16, Arc<Mutex<SocketAddr>>), Box<dyn Error>> {
    let first = tokio::net::lookup_host("localhost:0")
        .await?
        .next()
        .ok_or("localhost resolves to nothing")?;
    let listener = tokio::net::TcpListener::bind(first).await?;
    let port = listener.local_addr()?.port();
    let target = Arc::new(Mutex::new(target));
    let routed = target.clone();
    tokio::spawn(async move {
        while let Ok((mut inbound, _)) = listener.accept().await {
            let to = *routed.lock();
            tokio::spawn(async move {
                if let Ok(mut outbound) = tokio::net::TcpStream::connect(to).await {
                    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                }
            });
        }
    });
    Ok((port, target))
}

/// An agent holding an identity (in memory), its accounts under `store`.
pub async fn spawn_identity_agent(
    store: &std::path::Path,
) -> Result<(SocketAddr, JoinHandle<()>), Box<dyn Error>> {
    let bind: SocketAddr = format!("127.0.0.1:{}", common::get_free_port()).parse()?;
    let kernel = CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(
        bind,
        citadel_internal_service::SERVER_RECONNECT,
        common::test_transfers(),
    )
    .await?
    .with_identity(IdentityConfig {
        store: Arc::new(MemoryIdentityStore::default()),
        well_known: Arc::new(NoFetch),
        codec: Arc::new(JsonCodec),
        unlock_idle: Duration::from_secs(3600),
        sign_limits: SignLimits::PROTOCOL,
        clock: Arc::new(MonotonicClock),
    });
    let mut builder = NodeBuilder::<StackedRatchet>::default();
    let node = common::test_stun_servers()
        .apply(&mut builder)
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

pub type Sink = WrappedSink<TcpIOInterface>;

pub async fn open(agent: SocketAddr) -> Result<(Sink, Stream), Box<dyn Error>> {
    Ok(InternalServiceConnector::connect(agent).await?.split())
}

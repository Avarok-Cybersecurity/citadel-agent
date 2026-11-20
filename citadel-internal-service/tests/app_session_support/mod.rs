//! Helpers for `an_app_session_reconnects_under_its_cid.rs`: in-process Citadel servers that
//! answer the identity challenge, the workspace server's grant question and (the app's server)
//! echo every other frame; a front that can cut every link; and an agent with an identity and
//! app egress, which reaches the app server as a pinned one.
//!
//! The grant question and answer are JSON this test defines (`{"q":[install,tag]}`,
//! `{"grant":{…}}`): the shipped codec is the workspace protocol's, in the parent repository.

use citadel_identity::{canonical_domain, fingerprint, CanonicalDomain, ServerKey};
use citadel_internal_service::kernel::identity::{
    IdentityConfig, MemoryIdentityStore, MonotonicClock, SignLimits,
};
use citadel_internal_service::kernel::CitadelWorkspaceService;
use citadel_internal_service::{
    AppNetConfig, AppSessionConfig, AuthoritySignal, HostNetAuthority, KernelGrant, SessionLimits,
};
use citadel_internal_service_test_common as common;
use citadel_internal_service_test_common::node_settings::TestNode;
use citadel_sdk::prefabs::server::client_connect_listener::ClientConnectListenerKernel;
use citadel_sdk::prelude::*;
use futures::StreamExt;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

pub const APP_ID: &str = "work.synchronized.app";
pub const ECHO: &[u8] = b"echo:";

/// One in-process server: the name it signs under and every CID it accepted, in order.
pub struct Node {
    pub domain: String,
    pub seed: [u8; 32],
    pub accepted: Arc<Mutex<Vec<u64>>>,
}

impl Node {
    pub fn fp(&self) -> String {
        fingerprint(ServerKey::from_seed(&self.seed).public_key())
    }
}

async fn first_localhost() -> Result<SocketAddr, Box<dyn Error>> {
    Ok(tokio::net::lookup_host("localhost:0")
        .await?
        .next()
        .ok_or("localhost resolves to nothing")?)
}

fn answer(
    frame: &[u8],
    cid: u64,
    key: &ServerKey,
    domain: &CanonicalDomain,
    grants: &[String],
) -> Vec<u8> {
    let value: Value = serde_json::from_slice(frame).unwrap_or(Value::Null);
    if let Some(nonce) = value["Request"]["ServerIdentityChallenge"]["nonce"].as_array() {
        let nonce: Vec<u8> = nonce
            .iter()
            .filter_map(Value::as_u64)
            .map(|n| n as u8)
            .collect();
        let nonce: [u8; 32] = nonce.try_into().expect("a 32-byte nonce");
        let reply = json!({"Response": {"ServerIdentity": {
            "domain": domain.as_str(),
            "pk": key.public_key().as_bytes().to_vec(),
            "sig": key.sign_challenge(domain, cid, &nonce),
        }}});
        return serde_json::to_vec(&reply).unwrap();
    }
    if let Some(q) = value["q"].as_array() {
        let reply = json!({"grant": {"install_id": q[0], "tag": q[1], "servers": grants}});
        return serde_json::to_vec(&reply).unwrap();
    }
    [ECHO, frame].concat()
}

/// A server on `listener`, signing as `localhost:<named_port>`, granting `grants` to every
/// install that asks.
pub fn spawn_node(
    listener: std::net::TcpListener,
    named_port: u16,
    seed: [u8; 32],
    grants: Vec<String>,
) -> Result<Node, Box<dyn Error>> {
    let domain = format!("localhost:{named_port}");
    let canonical = canonical_domain(&domain)?;
    let accepted = Arc::new(Mutex::new(Vec::new()));
    let seen = accepted.clone();
    let kernel = ClientConnectListenerKernel::new(
        move |conn: CitadelClientServerConnection<StackedRatchet>| {
            let (canonical, grants, seen) = (canonical.clone(), grants.clone(), seen.clone());
            async move {
                let cid = conn.cid;
                seen.lock().push(cid);
                let key = ServerKey::from_seed(&seed);
                let (mut sink, mut stream) = conn.split();
                while let Some(frame) = stream.next().await {
                    sink.send(answer(frame.as_ref(), cid, &key, &canonical, &grants))
                        .await?;
                }
                Ok(())
            }
        },
    );
    let (server, _) = common::server_test_node_on_listener(
        Box::new(kernel) as Box<dyn NetKernel<StackedRatchet>>,
        listener,
        |builder| {
            // A V2 app account's name, `<id_fp>.app-<install>@<server>`, is longer than the
            // SDK's default 37 bytes; a server that hosts apps raises the limit (the identity's
            // own bound is 255).
            let mut misc = common::pq::pq_server_misc();
            misc.credential_requirements.max_username_length = u8::MAX;
            builder.with_server_misc_settings(misc);
        },
    );
    tokio::spawn(async move {
        let _ = server.await;
    });
    Ok(Node {
        domain,
        seed,
        accepted,
    })
}

/// A server reached directly at its own port.
pub async fn spawn_direct(seed: [u8; 32], grants: Vec<String>) -> Result<Node, Box<dyn Error>> {
    let listener = std::net::TcpListener::bind(first_localhost().await?)?;
    let port = listener.local_addr()?.port();
    spawn_node(listener, port, seed, grants)
}

/// A server reached only through a front whose `cut` resets every live link at once.
pub async fn spawn_behind_front(
    seed: [u8; 32],
) -> Result<(Node, watch::Sender<u64>), Box<dyn Error>> {
    let front = tokio::net::TcpListener::bind(first_localhost().await?).await?;
    let port = front.local_addr()?.port();
    let listener = std::net::TcpListener::bind(first_localhost().await?)?;
    let upstream = listener.local_addr()?;
    let node = spawn_node(listener, port, seed, Vec::new())?;
    let (cut, _) = watch::channel(0u64);
    let cuts = cut.clone();
    tokio::spawn(async move {
        while let Ok((mut inbound, _)) = front.accept().await {
            let mut cut = cuts.subscribe();
            cut.mark_unchanged();
            tokio::spawn(async move {
                let Ok(mut outbound) = tokio::net::TcpStream::connect(upstream).await else {
                    return;
                };
                tokio::select! {
                    _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound) => {}
                    _ = cut.changed() => {}
                }
            });
        }
    });
    Ok((node, cut))
}

struct JsonAuthority;

impl HostNetAuthority for JsonAuthority {
    fn grant_query(&self, install_id: &str, tag: u64) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&json!({"q": [install_id, tag]})).map_err(|e| e.to_string())
    }

    fn classify(&self, frame: &[u8]) -> AuthoritySignal {
        let Ok(value) = serde_json::from_slice::<Value>(frame) else {
            return AuthoritySignal::Unrelated;
        };
        let grant = &value["grant"];
        let (Some(install_id), Some(tag)) = (grant["install_id"].as_str(), grant["tag"].as_u64())
        else {
            return AuthoritySignal::Unrelated;
        };
        let servers = grant["servers"].as_array().into_iter().flatten();
        AuthoritySignal::Grant {
            install_id: install_id.to_string(),
            tag,
            grant: Some(KernelGrant {
                app_id: APP_ID.to_string(),
                hosts: Vec::new(),
                private: false,
                servers: servers
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect(),
                session: Some(SessionLimits {
                    max_msg_bytes: 4096,
                    max_msgs_per_s: 8,
                }),
            }),
        }
    }
}

/// An agent holding an identity, asking the workspace server about installs, with `app` a
/// pinned server (in-process servers speak TCP, not the `wss://` a well-known document names);
/// its accounts under `store`.
pub async fn spawn_agent(
    app: &Node,
    store: &std::path::Path,
) -> Result<SocketAddr, Box<dyn Error>> {
    let bind: SocketAddr = format!("127.0.0.1:{}", common::get_free_port()).parse()?;
    let kernel = CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(
        bind,
        citadel_internal_service::SERVER_RECONNECT,
        common::test_transfers(),
    )
    .await?
    .with_identity(IdentityConfig {
        store: Arc::new(MemoryIdentityStore::default()),
        well_known: Arc::new(crate::identity::NoFetch),
        codec: Arc::new(crate::identity::JsonCodec),
        unlock_idle: Duration::from_secs(3600),
        sign_limits: SignLimits::PROTOCOL,
        clock: Arc::new(MonotonicClock),
    })
    .with_app_net(
        AppNetConfig {
            page_origin_hosts: vec!["app.test.invalid".to_string()],
            agent_port: bind.port(),
        },
        Arc::new(JsonAuthority),
    )
    .with_app_sessions(AppSessionConfig {
        pinned_servers: [(app.domain.clone(), app.fp())].into(),
    });
    let mut builder = NodeBuilder::<StackedRatchet>::default();
    let node = common::test_stun_servers()
        .apply(&mut builder)
        .with_backend(BackendType::Filesystem(
            store.to_string_lossy().into_owned(),
        ))
        .with_node_type(NodeType::Peer)
        .as_test_agent(&kernel)
        .build(kernel)?;
    tokio::spawn(async move {
        let _ = node.await;
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(bind)
}

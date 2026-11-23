//! An account outlives the server process it registered on: a server that stops and starts
//! again over the same store and the same sign-in settings (what a hosted workspace does when
//! its host restarts it) still signs the account in, and the agent's reconnect brings the
//! session back by itself.
//!
//! Every other reconnect test cuts the link and leaves the server running, so none of them
//! could tell an account the store holds from one the running process remembers.
//!
//! The control is a server that comes back with another OPRF seed: it holds the account and
//! can no longer verify its password, so it answers "Authentication failed" to a sign-in that
//! is correct. The agent must stop at that answer, not repeat it for ten minutes.

#[allow(dead_code)]
#[path = "reconnect_support/mod.rs"]
mod reconnect;
#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service_test_common::node_settings::TestNode;
use citadel_internal_service_test_common::pq::{spawn_agent, username, PASSWORD, RP_ID};
use citadel_internal_service_test_common::pq_accounts::password_offer;
use citadel_internal_service_test_common::pq_window::Window;
use citadel_internal_service_test_common::{server_test_node_on_listener, setup_log};
use citadel_sdk::prelude::*;
use citadel_user::auth::pq::oprf::OprfSeed;
use citadel_user::auth::pq::record::KsfParams;
use citadel_user::auth::pq::server::PqAuthServerSettings;
use parking_lot::Mutex;
use reconnect::next_link_event;
use std::error::Error;
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::time::Duration;

type Remote = Arc<Mutex<Option<NodeRemote<StackedRatchet>>>>;

/// A server kernel that does nothing but keep its remote, so the test can stop the node.
struct Stoppable(Remote);

#[async_trait]
impl NetKernel<StackedRatchet> for Stoppable {
    fn load_remote(&mut self, remote: NodeRemote<StackedRatchet>) -> Result<(), NetworkError> {
        *self.0.lock() = Some(remote);
        Ok(())
    }

    async fn on_start(&self) -> Result<(), NetworkError> {
        Ok(())
    }

    async fn on_node_event_received(
        &self,
        _event: NodeResult<StackedRatchet>,
    ) -> Result<(), NetworkError> {
        Ok(())
    }

    async fn on_stop(&mut self) -> Result<(), NetworkError> {
        Ok(())
    }
}

struct Running {
    remote: Remote,
    task: tokio::task::JoinHandle<()>,
}

/// What a restart keeps: the store, the OPRF seed and where the server listens.
struct Host {
    store: String,
    seed: [u8; 32],
}

impl Host {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("citadel-restart-{}", uuid::Uuid::new_v4()));
        Self {
            store: dir.to_string_lossy().into_owned(),
            seed: [0x5a; 32],
        }
    }

    fn sign_in(&self) -> ServerMiscSettings {
        let settings = PqAuthServerSettings::new(
            OprfSeed::from_bytes(self.seed),
            KsfParams::FLOOR,
            ThrottlePolicy::RECOMMENDED,
        )
        .and_then(|settings| settings.with_security_key_rp_id(RP_ID))
        .expect("the floor parameters and the relying party are accepted");
        ServerMiscSettings {
            pq_sign_in: Some(settings),
            ..Default::default()
        }
    }

    /// A new server process over this host's store, listening on `listener`.
    fn start(&self, listener: TcpListener) -> (Running, SocketAddr) {
        let remote = Remote::default();
        let (node, addr) = server_test_node_on_listener(Stoppable(remote.clone()), listener, |b| {
            let _ = b
                .with_backend(BackendType::Filesystem(self.store.clone()))
                .with_server_misc_settings(self.sign_in())
                .as_test_node();
        });
        let task = tokio::spawn(async move {
            let _ = node.await;
        });
        (Running { remote, task }, addr)
    }
}

async fn stop(running: Running) {
    let remote = running.remote.lock().clone().expect("a running node");
    remote.shutdown().await.expect("shutdown");
    tokio::time::timeout(Duration::from_secs(30), running.task)
        .await
        .expect("the node did not stop")
        .expect("node task");
}

/// Where the agent dials for the whole test: the account is registered to one address, and a
/// restarted process listens on a port of its own, so this forwards to whichever runs now.
struct Door {
    addr: SocketAddr,
    upstream: Arc<Mutex<SocketAddr>>,
}

impl Door {
    async fn open(upstream: SocketAddr) -> Result<Self, Box<dyn Error>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let upstream = Arc::new(Mutex::new(upstream));
        let current = upstream.clone();
        tokio::spawn(async move {
            while let Ok((mut inbound, _)) = listener.accept().await {
                let to = *current.lock();
                tokio::spawn(async move {
                    if let Ok(mut outbound) = tokio::net::TcpStream::connect(to).await {
                        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                    }
                });
            }
        });
        Ok(Self { addr, upstream })
    }

    fn lead_to(&self, upstream: SocketAddr) {
        *self.upstream.lock() = upstream;
    }
}

/// A fresh listener for a server process.
fn listener() -> Result<TcpListener, std::io::Error> {
    TcpListener::bind("127.0.0.1:0")
}

#[tokio::test]
async fn a_fresh_sign_in_works_on_the_restarted_server() -> Result<(), Box<dyn Error>> {
    setup_log();
    let host = Host::new();
    let (first, upstream) = host.start(listener()?);
    let door = Door::open(upstream).await?;
    let addr = door.addr;
    let mut window = Window::open(spawn_agent().await?).await?;
    let name = username("restart");
    let registered = window.register(addr, &name, PASSWORD).await??;

    stop(first).await;
    let (_second, upstream) = host.start(listener()?);
    door.lead_to(upstream);

    let cid = window.sign_in(addr, &name, &password_offer()).await??;
    assert_eq!(cid, registered.cid);
    Ok(())
}

#[tokio::test]
async fn a_signed_in_session_comes_back_after_its_server_restarts() -> Result<(), Box<dyn Error>> {
    setup_log();
    let host = Host::new();
    let (first, upstream) = host.start(listener()?);
    let door = Door::open(upstream).await?;
    let addr = door.addr;
    let mut window = Window::open(spawn_agent().await?).await?;
    let name = username("restart");
    let _ = window.register(addr, &name, PASSWORD).await??;
    let cid = window.sign_in(addr, &name, &password_offer()).await??;

    stop(first).await;
    let (_second, upstream) = host.start(listener()?);
    door.lead_to(upstream);

    assert_eq!(
        link_events_to_the_end(&mut window, cid).await?,
        [
            "lost(reconnecting=true)".to_string(),
            "reconnected".to_string()
        ]
    );
    Ok(())
}

/// What the window hears of `cid`'s link from a drop until it is back or gone.
async fn link_events_to_the_end(
    window: &mut Window,
    cid: u64,
) -> Result<Vec<String>, Box<dyn Error>> {
    let mut events = vec![next_link_event(&mut window.stream, cid).await?];
    while !matches!(
        events.last().map(String::as_str),
        Some("reconnected" | "disconnected")
    ) {
        events.push(next_link_event(&mut window.stream, cid).await?);
    }
    Ok(events)
}

#[tokio::test]
async fn a_server_that_refuses_the_kept_password_ends_the_reconnect_at_once(
) -> Result<(), Box<dyn Error>> {
    setup_log();
    let mut host = Host::new();
    let (first, upstream) = host.start(listener()?);
    let door = Door::open(upstream).await?;
    let addr = door.addr;
    let mut window = Window::open(spawn_agent().await?).await?;
    let name = username("restart");
    let _ = window.register(addr, &name, PASSWORD).await??;
    let cid = window.sign_in(addr, &name, &password_offer()).await??;

    stop(first).await;
    host.seed = [0xa5; 32];
    let (_second, upstream) = host.start(listener()?);
    door.lead_to(upstream);

    let started = std::time::Instant::now();
    assert_eq!(
        link_events_to_the_end(&mut window, cid).await?,
        [
            "lost(reconnecting=true)".to_string(),
            "failed(Authentication failed)".to_string(),
            "disconnected".to_string(),
        ]
    );
    // One attempt, after the first wait: nowhere near a second one's backoff.
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    Ok(())
}

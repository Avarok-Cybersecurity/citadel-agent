//! A server that stops and starts again over the same store and the same sign-in settings, as
//! a hosted workspace does when its host restarts it, and the door the agent keeps dialling.

use crate::reconnect::next_link_event;
use citadel_internal_service_test_common::node_settings::TestNode;
use citadel_internal_service_test_common::pq::RP_ID;
use citadel_internal_service_test_common::pq_window::Window;
use citadel_internal_service_test_common::server_test_node_on_listener;
use citadel_sdk::prelude::*;
use citadel_user::auth::pq::oprf::OprfSeed;
use citadel_user::auth::pq::record::KsfParams;
use citadel_user::auth::pq::server::PqAuthServerSettings;
use parking_lot::Mutex;
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

pub struct Running {
    remote: Remote,
    task: tokio::task::JoinHandle<()>,
}

/// What a restart keeps: the store, the OPRF seed and where the server listens.
pub struct Host {
    store: String,
    pub seed: [u8; 32],
    /// The longest a resume credential lives on this host; `None` issues none.
    pub resume: Option<Duration>,
}

impl Host {
    pub fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("citadel-restart-{}", uuid::Uuid::new_v4()));
        Self {
            store: dir.to_string_lossy().into_owned(),
            seed: [0x5a; 32],
            resume: None,
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
        let settings = match self.resume {
            Some(max) => settings.with_resume(ResumePolicy::new(max).expect("a maximum")),
            None => settings,
        };
        ServerMiscSettings {
            pq_sign_in: Some(settings),
            ..Default::default()
        }
    }

    /// A new server process over this host's store, listening on `listener`.
    pub fn start(&self, listener: TcpListener) -> (Running, SocketAddr) {
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

pub async fn stop(running: Running) {
    let remote = running.remote.lock().clone().expect("a running node");
    remote.shutdown().await.expect("shutdown");
    tokio::time::timeout(Duration::from_secs(30), running.task)
        .await
        .expect("the node did not stop")
        .expect("node task");
}

/// Where the agent dials for the whole test: the account is registered to one address, and a
/// restarted process listens on a port of its own, so this forwards to whichever runs now.
pub struct Door {
    pub addr: SocketAddr,
    upstream: Arc<Mutex<SocketAddr>>,
}

impl Door {
    pub async fn open(upstream: SocketAddr) -> Result<Self, Box<dyn Error>> {
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

    pub fn lead_to(&self, upstream: SocketAddr) {
        *self.upstream.lock() = upstream;
    }
}

/// A fresh listener for a server process.
pub fn listener() -> Result<TcpListener, std::io::Error> {
    TcpListener::bind("127.0.0.1:0")
}

/// What the window hears of `cid`'s link from a drop until it is back or gone.
pub async fn link_events_to_the_end(
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

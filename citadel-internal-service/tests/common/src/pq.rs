//! A workspace server with post-quantum sign-in, and an agent to sign in to it through.
//!
//! Each server gets a fresh OPRF seed and the Argon2id floor the SDK accepts, so a password
//! sign-in costs the client one floor-strength hash. The server's own Argon2 settings are
//! poisoned (zero lanes), as in the SDK's suites: a post-quantum server never runs Argon2, and
//! if it tried, these tests would fail.

use crate::{server_test_node_skip_cert_verification, test_backend, test_stun_servers};
use citadel_internal_service::kernel::CitadelWorkspaceService;
use citadel_internal_service::SERVER_RECONNECT;
use citadel_sdk::prefabs::server::empty::EmptyKernel;
use citadel_sdk::prelude::*;
use citadel_user::auth::pq::admission::{
    AdmissionContext, AdmissionPolicy, AdmissionRefusal, AdmissionToken,
};
use citadel_user::auth::pq::oprf::OprfSeed;
use citadel_user::auth::pq::record::KsfParams;
use citadel_user::auth::pq::server::PqAuthServerSettings;
use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use uuid::Uuid;

pub const PASSWORD: &str = "correct horse battery";
/// The WebAuthn credential id of the tests' security key.
pub const CRED: &[u8] = b"yubikey-5-credential";

/// Which sign-in the server runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Server {
    PostQuantum,
    /// A server without post-quantum settings: the legacy Argon2 sign-in, as a server from
    /// before protocol 0.12 runs it.
    Legacy,
}

/// Spawns the server and returns where it listens.
pub fn spawn_server(kind: Server) -> SocketAddr {
    spawn(kind, None)
}

/// A post-quantum server that requires an admission token for every fresh sign-in and
/// registration, and admits exactly `GOOD_TOKEN`: a stand-in for Turnstile.
pub fn spawn_guarded_server() -> SocketAddr {
    spawn(Server::PostQuantum, Some(Arc::new(Turnstile)))
}

pub const GOOD_TOKEN: &str = "turnstile-ok";

struct Turnstile;

#[citadel_sdk::async_trait]
impl AdmissionPolicy for Turnstile {
    async fn admit(&self, ctx: AdmissionContext) -> Result<(), AdmissionRefusal> {
        match ctx.token.as_ref().map(AdmissionToken::as_str) {
            None => Err(AdmissionRefusal::Required),
            Some(GOOD_TOKEN) => Ok(()),
            Some(_) => Err(AdmissionRefusal::Failed("invalid-input-response".into())),
        }
    }
}

fn spawn(kind: Server, admission: Option<Arc<dyn AdmissionPolicy>>) -> SocketAddr {
    let (node, addr) = server_test_node_skip_cert_verification(
        EmptyKernel::<StackedRatchet>::default(),
        |builder| {
            if kind == Server::PostQuantum {
                let settings = PqAuthServerSettings::new(OprfSeed::generate(), KsfParams::FLOOR)
                    .expect("the floor parameters are accepted");
                let _ = builder
                    .with_server_misc_settings(ServerMiscSettings {
                        pq_sign_in: Some(settings),
                        admission,
                        ..Default::default()
                    })
                    .with_server_argon_settings(ArgonDefaultServerSettings {
                        lanes: 0,
                        ..Default::default()
                    });
            }
        },
    );
    drop(tokio::spawn(node));
    addr
}

/// Spawns an agent with a fresh store and returns its localhost address. A window can open it
/// at once: the socket is bound before this returns, and opening waits for the greeting.
pub async fn spawn_agent() -> Result<SocketAddr, Box<dyn Error>> {
    let bind: SocketAddr = format!("127.0.0.1:{}", crate::get_free_port()).parse()?;
    let kernel =
        CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(bind, SERVER_RECONNECT).await?;
    let node = test_stun_servers()
        .apply(&mut NodeBuilder::<StackedRatchet>::default())
        .with_backend(test_backend())
        .with_node_type(NodeType::Peer)
        .with_insecure_skip_cert_verification()
        .build(kernel)?;
    drop(tokio::spawn(node));
    Ok(bind)
}

pub fn username(tag: &str) -> String {
    format!("{tag}.{}", &Uuid::new_v4().to_string()[..8])
}

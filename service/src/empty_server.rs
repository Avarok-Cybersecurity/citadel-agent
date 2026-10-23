use citadel_internal_service::stun::{StunServers, STUN_SERVERS_ENV};
use citadel_sdk::prefabs::server::empty::EmptyKernel;
use citadel_sdk::prelude::{BackendType, NodeBuilder, NodeType, StackedRatchet};
use std::error::Error;
use std::net::SocketAddr;
use structopt::StructOpt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    citadel_sdk::logging::setup_log();
    let opts: Options = Options::from_args();
    // Required, as for the agent: the SDK has no default STUN list.
    let stun_servers = StunServers::resolve(
        std::env::var(STUN_SERVERS_ENV).ok().as_deref(),
        opts.stun_servers.as_deref(),
    )?;
    let service = EmptyKernel::<StackedRatchet>::default();
    let mut builder = NodeBuilder::default();
    let mut builder = stun_servers
        .apply(&mut builder)
        .with_backend(BackendType::InMemory)
        .with_node_type(NodeType::server(opts.bind)?)
        .with_path_policy(citadel_internal_service::path_policy::every_path_policy()?);

    if opts.dangerous.unwrap_or(false) {
        builder = builder.with_insecure_skip_cert_verification()
    }

    builder.build(service)?.await?;

    Ok(())
}

#[derive(Debug, StructOpt)]
#[structopt(
    name = "citadel-service-bin",
    about = "Used for running a local service for citadel applications"
)]
struct Options {
    #[structopt(short, long)]
    bind: SocketAddr,
    #[structopt(short, long)]
    dangerous: Option<bool>,
    /// Three comma-separated STUN servers as host:port (no scheme). Required;
    /// `INTERNAL_SERVICE_STUN_SERVERS` env var overrides this.
    #[structopt(long)]
    stun_servers: Option<String>,
}

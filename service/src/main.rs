use citadel_internal_service::kernel::CitadelWorkspaceService;
use citadel_internal_service::sweep_stale_browser_transfers;
use citadel_sdk::prelude::{BackendType, NodeBuilder, NodeType, StackedRatchet};
use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use structopt::StructOpt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    citadel_sdk::logging::setup_log();

    // One-shot startup sweep of the shared browser-transfer temp root.
    // Per-request cleanup tasks are cancelled on process exit, so a
    // crash before the 10-min TTL fires leaks the materialised payload
    // file; without this sweep, repeated crashes accumulate orphaned
    // upload bytes unboundedly. Safe to run before the runtime is
    // built because the helper uses blocking `std::fs`.
    sweep_stale_browser_transfers();

    // Initialize deadlock detector if feature is enabled
    #[cfg(feature = "deadlock-detection")]
    {
        let _ = *DEADLOCK_INIT;
    }

    let opts: Options = Options::from_args();
    let service = CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(opts.bind).await?;

    // Resolve the SDK backend from CLI + env (env takes precedence so docker
    // operators can flip backends without rebuilding). `filesystem` is required
    // for file transfer to function — the SDK refuses `SendObject` calls when
    // both peers run on `InMemory`, with the error
    //   "File transfer is not enabled for this p2p session.
    //    Both nodes must use a filesystem backend"
    // Default stays `in-memory` for `tilt`-style ephemeral dev runs.
    let backend = resolve_backend(&opts)?;

    let mut builder = NodeBuilder::default();
    let mut builder = builder.with_backend(backend).with_node_type(NodeType::Peer);

    if opts.dangerous.unwrap_or(false) {
        builder = builder.with_insecure_skip_cert_verification()
    }

    builder.build(service)?.await?;

    Ok(())
}

fn resolve_backend(opts: &Options) -> Result<BackendType, Box<dyn Error>> {
    let env_kind = std::env::var("INTERNAL_SERVICE_BACKEND").ok();
    let env_dir = std::env::var("INTERNAL_SERVICE_DATA_DIR")
        .ok()
        .map(PathBuf::from);

    let kind = env_kind
        .as_deref()
        .or(opts.backend.as_deref())
        .unwrap_or("in-memory");
    let data_dir = env_dir.or_else(|| opts.data_dir.clone());

    match kind {
        "in-memory" | "inmemory" => Ok(BackendType::InMemory),
        "filesystem" | "fs" => {
            let path = data_dir.unwrap_or_else(|| PathBuf::from("./internal-service-data"));
            // Create the directory up front so the SDK doesn't fail on first write.
            std::fs::create_dir_all(&path)?;
            Ok(BackendType::Filesystem(path.to_string_lossy().into_owned()))
        }
        other => Err(format!(
            "Unknown backend kind {other:?}; expected 'in-memory' or 'filesystem'"
        )
        .into()),
    }
}

#[derive(Debug, StructOpt)]
#[structopt(
    name = "internal-service",
    about = "Used for running a local service for citadel applications"
)]
struct Options {
    #[structopt(short, long)]
    bind: SocketAddr,
    #[structopt(short, long)]
    dangerous: Option<bool>,
    /// Account storage backend: "in-memory" (default, ephemeral) or "filesystem".
    /// `INTERNAL_SERVICE_BACKEND` env var overrides this.
    #[structopt(long)]
    backend: Option<String>,
    /// Directory used by the filesystem backend. Defaults to
    /// `./internal-service-data`. Ignored when backend is "in-memory".
    /// `INTERNAL_SERVICE_DATA_DIR` env var overrides this.
    #[structopt(long, parse(from_os_str))]
    data_dir: Option<PathBuf>,
}

#[cfg(feature = "deadlock-detection")]
lazy_static::lazy_static! {
    static ref DEADLOCK_INIT: () = {
        let _ = std::thread::spawn(move || {
            info!(target: "gadget", "Executing deadlock detector ...");
            use std::thread;
            use std::time::Duration;
            use parking_lot::deadlock;
            use citadel_sdk::logging::*;
            loop {
                std::thread::sleep(Duration::from_secs(5));
                let deadlocks = deadlock::check_deadlock();
                if deadlocks.is_empty() {
                    continue;
                }

                error!(target: "citadel", "{} deadlocks detected", deadlocks.len());
                for (i, threads) in deadlocks.iter().enumerate() {
                    error!(target: "citadel", "Deadlock #{}", i);
                    for t in threads {
                        error!(target: "citadel", "Thread Id {:#?}", t.thread_id());
                        error!(target: "citadel", "{:#?}", t.backtrace());
                    }
                }
            }
        });
    };
}

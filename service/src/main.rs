use citadel_internal_service::kernel::CitadelWorkspaceService;
use citadel_internal_service::sweep_stale_browser_transfers;
use citadel_sdk::prelude::{BackendType, NodeBuilder, NodeType, StackedRatchet};
use std::error::Error;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
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

    // The in-memory backend is convenient for ephemeral dev runs but silently
    // disables P2P file transfer (the SDK refuses `SendObject` unless both
    // peers use a filesystem backend) and loses all account state on restart.
    // Surface that loudly at startup so an operator who forgot to set
    // `INTERNAL_SERVICE_BACKEND=filesystem` in production isn't left guessing
    // why uploads fail.
    if matches!(backend, BackendType::InMemory) {
        citadel_sdk::logging::warn!(
            target: "citadel",
            "Internal service starting with the IN-MEMORY backend: account state is \
             ephemeral and P2P file transfer is DISABLED. Set \
             INTERNAL_SERVICE_BACKEND=filesystem (with INTERNAL_SERVICE_DATA_DIR) to \
             enable persistence and file transfer."
        );
    }

    let mut builder = NodeBuilder::default();
    let mut builder = builder.with_backend(backend).with_node_type(NodeType::Peer);

    if opts.dangerous.unwrap_or(false) {
        builder = builder.with_insecure_skip_cert_verification()
    }

    builder.build(service)?.await?;

    Ok(())
}

/// The resolved backend decision, separated from any filesystem side effect
/// so the precedence/alias/validation logic can be unit-tested in isolation.
#[derive(Debug, PartialEq, Eq)]
enum BackendChoice {
    InMemory,
    Filesystem(PathBuf),
}

/// Pure resolution of the backend choice from the env + CLI inputs. Env wins
/// over CLI (so docker operators can flip backends without rebuilding). No
/// I/O happens here — directory creation is the caller's job.
fn choose_backend(
    env_kind: Option<&str>,
    env_dir: Option<PathBuf>,
    opts_backend: Option<&str>,
    opts_data_dir: Option<PathBuf>,
) -> Result<BackendChoice, String> {
    let kind = env_kind.or(opts_backend).unwrap_or("in-memory");
    let data_dir = env_dir.or(opts_data_dir);

    match kind {
        "in-memory" | "inmemory" => Ok(BackendChoice::InMemory),
        "filesystem" | "fs" => {
            Ok(BackendChoice::Filesystem(data_dir.unwrap_or_else(|| {
                PathBuf::from("./internal-service-data")
            })))
        }
        other => Err(format!(
            "Unknown backend kind {other:?}; expected 'in-memory' or 'filesystem'"
        )),
    }
}

fn resolve_backend(opts: &Options) -> Result<BackendType, Box<dyn Error>> {
    let env_kind = std::env::var("INTERNAL_SERVICE_BACKEND").ok();
    let env_dir = std::env::var("INTERNAL_SERVICE_DATA_DIR")
        .ok()
        .map(PathBuf::from);

    match choose_backend(
        env_kind.as_deref(),
        env_dir,
        opts.backend.as_deref(),
        opts.data_dir.clone(),
    )? {
        BackendChoice::InMemory => Ok(BackendType::InMemory),
        BackendChoice::Filesystem(path) => {
            // Create the directory up front so the SDK doesn't fail on first
            // write. The filesystem backend stores sensitive account/node
            // state, so make it private (0700) rather than umask-default
            // (typically 0755, world-readable) on multi-user hosts.
            create_private_data_dir(&path)?;
            Ok(BackendType::Filesystem(path.to_string_lossy().into_owned()))
        }
    }
}

/// Create (or tighten) the filesystem-backend data directory as a private
/// `0700` directory on Unix. Tightening an already-existing directory is safe
/// because the service owns its own data dir. Falls back to the platform
/// default elsewhere.
///
/// If the path already exists it must be a real directory we own — never a
/// symlink. Following a pre-planted symlink would make `set_permissions`
/// chmod (and the SDK then populate) an attacker-controlled target (CWE-59),
/// redirecting sensitive account/node state.
fn create_private_data_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        match std::fs::symlink_metadata(path) {
            Ok(meta) => {
                if meta.file_type().is_symlink() || !meta.is_dir() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!(
                            "backend data dir {path:?} is a symlink or not a directory; refusing"
                        ),
                    ));
                }
                let our_euid = unsafe { libc::geteuid() };
                if meta.uid() != our_euid {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!(
                            "backend data dir {path:?} is owned by uid {} but we are {our_euid}; refusing",
                            meta.uid()
                        ),
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Create parents (may be a fresh mount) then the final
                // component, both at 0700.
                if let Some(parent) = path.parent() {
                    std::fs::DirBuilder::new()
                        .recursive(true)
                        .mode(0o700)
                        .create(parent)?;
                }
                std::fs::DirBuilder::new().mode(0o700).create(path)?;
            }
            Err(e) => return Err(e),
        }
        // Tighten an existing (verified real, owned) directory.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path)
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

#[cfg(test)]
mod tests {
    use super::{choose_backend, BackendChoice};
    use std::path::PathBuf;

    #[test]
    fn defaults_to_in_memory() {
        assert_eq!(
            choose_backend(None, None, None, None).unwrap(),
            BackendChoice::InMemory
        );
    }

    #[test]
    fn cli_filesystem_with_explicit_data_dir() {
        assert_eq!(
            choose_backend(
                None,
                None,
                Some("filesystem"),
                Some(PathBuf::from("/srv/data"))
            )
            .unwrap(),
            BackendChoice::Filesystem(PathBuf::from("/srv/data"))
        );
    }

    #[test]
    fn filesystem_falls_back_to_default_dir() {
        assert_eq!(
            choose_backend(None, None, Some("filesystem"), None).unwrap(),
            BackendChoice::Filesystem(PathBuf::from("./internal-service-data"))
        );
    }

    #[test]
    fn env_kind_overrides_cli_kind() {
        // CLI asks for filesystem, env forces in-memory — env wins.
        assert_eq!(
            choose_backend(Some("in-memory"), None, Some("filesystem"), None).unwrap(),
            BackendChoice::InMemory
        );
    }

    #[test]
    fn env_dir_overrides_cli_dir() {
        assert_eq!(
            choose_backend(
                Some("filesystem"),
                Some(PathBuf::from("/env/dir")),
                Some("filesystem"),
                Some(PathBuf::from("/cli/dir")),
            )
            .unwrap(),
            BackendChoice::Filesystem(PathBuf::from("/env/dir"))
        );
    }

    #[test]
    fn accepts_short_aliases() {
        assert_eq!(
            choose_backend(Some("inmemory"), None, None, None).unwrap(),
            BackendChoice::InMemory
        );
        assert_eq!(
            choose_backend(Some("fs"), None, None, Some(PathBuf::from("/d"))).unwrap(),
            BackendChoice::Filesystem(PathBuf::from("/d"))
        );
    }

    #[test]
    fn unknown_kind_is_an_error() {
        let err = choose_backend(Some("sqlite"), None, None, None).unwrap_err();
        assert!(err.contains("Unknown backend kind"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn create_private_data_dir_creates_a_0700_dir() {
        use super::create_private_data_dir;
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("cid-datadir-ok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join("internal-service-data");
        create_private_data_dir(&dir).expect("should create the data dir");
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "data dir should be private 0700");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn create_private_data_dir_rejects_symlink_and_non_dir() {
        use super::create_private_data_dir;
        let base = std::env::temp_dir().join(format!("cid-datadir-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        // A symlink at the data-dir path must be refused (CWE-59).
        let target = base.join("target");
        std::fs::create_dir_all(&target).unwrap();
        let link = base.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let err = create_private_data_dir(&link).expect_err("symlink must be rejected");
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);

        // A regular file at the path must be refused too.
        let file = base.join("afile");
        std::fs::write(&file, b"x").unwrap();
        let err = create_private_data_dir(&file).expect_err("non-dir must be rejected");
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);

        let _ = std::fs::remove_dir_all(&base);
    }
}

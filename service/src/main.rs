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
    let data_dir_provided = env_dir.is_some() || opts.data_dir.is_some();

    match choose_backend(
        env_kind.as_deref(),
        env_dir,
        opts.backend.as_deref(),
        opts.data_dir.clone(),
    )? {
        BackendChoice::InMemory => {
            // A data dir with the in-memory backend is a no-op; warn loudly so
            // an operator who set INTERNAL_SERVICE_DATA_DIR but forgot to also
            // set INTERNAL_SERVICE_BACKEND=filesystem isn't surprised by data
            // loss on restart.
            if data_dir_provided {
                citadel_sdk::logging::warn!(
                    target: "citadel",
                    "A data directory was configured but the backend is IN-MEMORY, so it is \
                     IGNORED and all state is ephemeral. Set INTERNAL_SERVICE_BACKEND=filesystem \
                     to persist to the configured data directory."
                );
            }
            Ok(BackendType::InMemory)
        }
        BackendChoice::Filesystem(path) => {
            // `BackendType::Filesystem` carries a `String`, so a non-UTF-8 path
            // would be silently rewritten by `to_string_lossy()` (invalid bytes
            // become U+FFFD) — we'd validate/create one directory but hand the
            // SDK a *different*, mangled path, writing state somewhere
            // unexpected (or failing on first write). Reject non-UTF-8 up front
            // so the directory we validate and the path the backend uses are
            // always the same bytes.
            let path_str = path
                .to_str()
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("backend data dir {path:?} is not valid UTF-8; refusing"),
                    )
                })?
                .to_owned();
            // Create the directory up front so the SDK doesn't fail on first
            // write. The filesystem backend stores sensitive account/node
            // state, so make it private (0700) rather than umask-default
            // (typically 0755, world-readable) on multi-user hosts.
            create_private_data_dir(&path)?;
            Ok(BackendType::Filesystem(path_str))
        }
    }
}

/// Create (or tighten) the filesystem-backend data directory as a private
/// `0700` directory on Unix. Tightening an already-existing directory is safe
/// because the service owns its own data dir. Falls back to the platform
/// default elsewhere.
///
/// The filesystem backend stores sensitive account/node state, so the directory
/// must be private and must never be a symlink — following a pre-planted
/// symlink would redirect that state to an attacker-controlled target (CWE-59).
///
/// ## Race-free existing-directory handling
/// For a directory that already exists we open it with
/// `O_NOFOLLOW | O_DIRECTORY | O_CLOEXEC` and then verify ownership (`fstat`)
/// and tighten permissions (`fchmod`) **on the returned file descriptor** — the
/// path is never re-resolved between the check and the chmod. `O_NOFOLLOW`
/// makes the kernel refuse a symlink at the final component (ELOOP) and
/// `O_DIRECTORY` refuses a non-directory (ENOTDIR), so an attacker with write
/// access to the parent cannot swap the verified directory for a symlink in a
/// stat→chmod window (the CWE-59 TOCTOU this function guards against).
///
/// ## Parent chain
/// We deliberately do NOT walk/validate the full ancestor chain nor create it
/// recursively: the data dir is operator-supplied configuration (CLI flag /
/// env var), so the operator — or the container volume mount — owns the parent
/// chain. On first boot we require only the *immediate* parent to already exist
/// as a real, non-symlink directory and create just the final component
/// non-recursively at mode 0700, the proportionate CWE-59 mitigation for a
/// config-derived (not request-derived) path.
fn create_private_data_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
        use std::os::unix::io::AsRawFd;

        // Open the directory ITSELF and operate on the fd for every subsequent
        // check/mutation, so nothing re-resolves the path. `O_NOFOLLOW` makes a
        // symlink at `path` fail the open with ELOOP; `O_DIRECTORY` makes a
        // non-directory fail with ENOTDIR. Both surface in the catch-all arm.
        match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(path)
        {
            Ok(dir) => {
                // `File::metadata` is `fstat(fd)`: it describes the very inode
                // we hold open, not whatever the path resolves to now.
                let meta = dir.metadata()?;
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
                // `fchmod` the held fd, never the path → no stat→chmod TOCTOU.
                if unsafe { libc::fchmod(dir.as_raw_fd(), 0o700) } != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Don't recursively create the parent CHAIN — that would
                // follow a pre-planted intermediate symlink and populate the
                // backend under an attacker-controlled target. Require the
                // immediate parent to already exist (the operator / volume
                // mount provides it) and be a real, non-symlink directory,
                // then create ONLY the final component (non-recursive, 0700).
                //
                // `Path::new("data").parent()` is `Some("")` (an empty path),
                // NOT `None`, so a single-component *relative* data dir must map
                // the empty parent to "." — otherwise `symlink_metadata("")`
                // fails and the service refuses to start on a perfectly valid
                // `--data-dir data`. Only a truly parent-less path (e.g. "/",
                // which always exists and so never reaches this arm) yields
                // `None`, in which case "." is a harmless fallback.
                let parent = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                let pmeta = std::fs::symlink_metadata(parent).map_err(|e| {
                    std::io::Error::new(
                        e.kind(),
                        format!("backend data dir parent {parent:?} must already exist: {e}"),
                    )
                })?;
                if pmeta.file_type().is_symlink() || !pmeta.is_dir() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("backend data dir parent {parent:?} is a symlink or not a directory; refusing"),
                    ));
                }
                // Create the final component already-private. `DirBuilder`'s
                // mode is only masked by the umask, and 0o700 has no bits in the
                // umask's usual 0o077 range, so the result is reliably 0700 with
                // no follow-up chmod — which also avoids a create-then-chmod
                // TOCTOU on this first-boot path.
                std::fs::DirBuilder::new().mode(0o700).create(path)
            }
            // ELOOP (a symlink, refused by O_NOFOLLOW), ENOTDIR (a non-directory,
            // refused by O_DIRECTORY), EACCES, etc.: the path exists but is not a
            // private directory we can safely use. Fail closed.
            Err(e) => Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "backend data dir {path:?} is not a usable private directory ({e}); refusing"
                ),
            )),
        }
    }
    #[cfg(not(unix))]
    {
        // NOTE: non-Unix platforms (e.g. Windows) get only a best-effort
        // `create_dir_all` here — there is no private-ACL tightening and no
        // symlink/ownership validation, so the CWE-59 / private-permissions
        // guarantees of the Unix branch above do NOT hold. The internal service
        // is deployed on Linux (Docker); a hardened Windows path would need the
        // Win32 ACL APIs and is out of scope. Operators on non-Unix hosts must
        // ensure the data directory's parent is trusted and that filesystem
        // ACLs restrict access to the service's own account.
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
        // The immediate parent must already exist (we no longer create the
        // parent chain recursively); the volume mount provides it in prod.
        std::fs::create_dir_all(&base).unwrap();
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

    /// A single-component *relative* data dir (e.g. `--data-dir data`) has an
    /// empty `Path::parent()`, which must be treated as "." rather than making
    /// `symlink_metadata("")` fail and the service refuse to start. Regression
    /// test for that edge case.
    #[cfg(unix)]
    #[test]
    fn create_private_data_dir_accepts_single_component_relative_path() {
        use super::create_private_data_dir;
        use std::os::unix::fs::PermissionsExt;

        // Relative name created under the test's CWD; unique per process so
        // parallel test binaries don't collide. Removed by the guard on drop
        // (incl. panic-unwind) so we never leave junk in the crate dir.
        let rel = std::path::PathBuf::from(format!("cid-reldir-{}", std::process::id()));
        struct RelGuard(std::path::PathBuf);
        impl Drop for RelGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _ = std::fs::remove_dir_all(&rel);
        let _guard = RelGuard(rel.clone());

        create_private_data_dir(&rel)
            .expect("single-component relative data dir should be created");

        let meta = std::fs::symlink_metadata(&rel).expect("relative data dir should exist");
        assert!(meta.is_dir(), "relative data dir should be a directory");
        assert_eq!(
            meta.permissions().mode() & 0o777,
            0o700,
            "relative data dir should be 0700"
        );
    }

    /// An already-existing, loosely-permissioned data dir we own must be
    /// tightened to 0700 — this exercises the `open(O_NOFOLLOW)` + `fchmod`
    /// path (the race-free existing-directory branch), distinct from the
    /// first-boot create branch.
    #[cfg(unix)]
    #[test]
    fn create_private_data_dir_tightens_existing_loose_dir() {
        use super::create_private_data_dir;
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("cid-datadir-tighten-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        // A pre-existing world-readable/executable dir (mode 0755) we own.
        let dir = base.join("loose");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o755
        );

        create_private_data_dir(&dir).expect("existing dir should be tightened, not rejected");

        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700,
            "existing data dir should be tightened to 0700"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}

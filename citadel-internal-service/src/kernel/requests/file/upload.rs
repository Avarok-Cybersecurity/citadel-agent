use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    FileSource, InternalServiceRequest, InternalServiceResponse, SendFileRequestFailure,
    SendFileRequestSuccess,
};
use citadel_sdk::logging::{error, info, warn};
use citadel_sdk::prelude::{NetworkError, NodeRequest, Ratchet, SendObject, VirtualTargetType};
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

/// Maximum accepted payload size for `FileSource::ByteContents`.
///
/// Caps the RAM a single request can demand before any disk I/O happens.
/// The constant is chosen to be reachable from real callers (i.e. the
/// guard actually fires) rather than a number large enough to be defeated
/// by transport-layer framing earlier in the stack:
///
///   * The WebSocket/JSON transport (used by the browser UI) encodes
///     payloads via `serde_json::to_string`, where a `Vec<u8>` expands
///     by roughly 3-4x. The resulting JSON must fit within the WS frame
///     limit, so the largest raw `data.len()` that survives serialization
///     is somewhere near 16 MiB.
///   * The TCP transport uses `bincode2` (binary framing via
///     `SerializingCodec` over a 64 MiB `LengthDelimitedCodec`) and does
///     not incur the JSON expansion, but the cap is applied uniformly so
///     behaviour does not depend on which transport happens to be in use.
///   * The browser-side workspace UI applies a much stricter cap (a few
///     MiB) before invoking this path.
///
/// 16 MiB therefore sits at the natural ceiling of the WebSocket framing
/// layer while still being multiple orders of magnitude above any sane
/// browser upload. Larger transfers must use the native `PickFile` flow,
/// which streams the file from disk and bypasses both this cap and the
/// JSON expansion entirely.
const MAX_BYTE_CONTENTS_BYTES: usize = 16 * 1024 * 1024; // 16 MiB

/// Subdirectory under `std::env::temp_dir()` where browser-uploaded payloads
/// are materialised. Each request gets its own UUID-named subdirectory
/// containing one file (preserving the user's filename), removed by a
/// delayed-cleanup task scheduled when the file is created.
const BROWSER_TRANSFER_SUBDIR: &str = "citadel-browser-transfers";

/// How long a `ByteContents` temp file persists before the cleanup task
/// removes it.
///
/// The lifetime must outlive the SDK's `process_outbound_file` call - which
/// runs *after* `remote.send(...).await` resolves in this handler, on the
/// SDK's main node loop after dequeuing the request. Once the SDK has
/// `File::open`'d the path, the file may be unlinked safely on POSIX (the
/// inode persists for the open FD until close).
///
/// 10 minutes is comfortably longer than any realistic dequeue + open
/// latency under saturation, and bounds disk-leak from a crash to that
/// window even without an external sweeper. The bound matters because the
/// 16 MiB per-file cap (`MAX_BYTE_CONTENTS_BYTES`) keeps the worst-case
/// leak bounded in absolute terms.
const TEMP_FILE_TTL: Duration = Duration::from_secs(600);

/// Strip any directory components from a client-supplied file name so it can
/// be safely joined onto the temp dir path. A name like "photos/vacation.jpg"
/// would otherwise cause `std::fs::write` to fail cryptically because the
/// intermediate directory doesn't exist.
fn sanitize_file_name(raw: &str) -> String {
    Path::new(raw)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("upload")
        .to_string()
}

/// One-shot, best-effort sweep of the shared browser-transfer temp root.
/// Removes any per-request subdirectory older than `TEMP_FILE_TTL` so the
/// service doesn't accumulate orphaned payloads across crash-restart
/// cycles. The per-request `schedule_temp_dir_cleanup` task is cancelled
/// when the process exits, so without this every crash before the 10-min
/// timer fires leaks the materialised file; over many crashes the leak
/// is unbounded in count. The 16 MiB per-file cap bounds per-file size
/// but not aggregate disk usage.
///
/// Intended to run once at binary startup. The function uses blocking
/// `std::fs` intentionally so it does not require a tokio runtime — the
/// current call site at `service/src/main.rs` runs inside
/// `#[tokio::main]` but before any other tasks are spawned, so the
/// blocking I/O does not park a worker that another task is waiting
/// on. Errors are logged but not propagated — a sweep failure must
/// not block service startup.
pub fn sweep_stale_browser_transfers() {
    sweep_browser_transfers_in(
        &std::env::temp_dir().join(BROWSER_TRANSFER_SUBDIR),
        TEMP_FILE_TTL,
    );
}

/// Inner sweep that operates on an arbitrary root path. Split out so
/// tests can pass an isolated per-test directory rather than sharing
/// the production `$TMPDIR/citadel-browser-transfers/` root with
/// parallel test threads. Production callers go through
/// `sweep_stale_browser_transfers`.
fn sweep_browser_transfers_in(root: &Path, max_age: Duration) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        // The root only exists after the first upload; missing is the
        // normal first-boot state and not worth logging at warn level.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            warn!(
                target: "citadel",
                "Browser-transfer sweep: failed to read {:?}: {}", root, e
            );
            return;
        }
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        // `modified()` is unsupported on a handful of exotic filesystems;
        // when unavailable, fall back to deleting the entry on the
        // assumption that "no timestamp" means "older than we can
        // measure", which is safe for stale-cleanup purposes.
        let is_stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().unwrap_or_default() > max_age)
            .unwrap_or(true);
        if !is_stale {
            continue;
        }
        let path = entry.path();
        match std::fs::remove_dir_all(&path) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!(
                target: "citadel",
                "Browser-transfer sweep: failed to remove {:?}: {}", path, e
            ),
        }
    }
    if removed > 0 {
        info!(
            target: "citadel",
            "Browser-transfer sweep removed {} stale request dir(s) under {:?}",
            removed, root
        );
    }
}

/// Schedule a best-effort delayed cleanup of a per-request temp directory
/// (containing a single materialised payload file).
///
/// We do NOT unlink at handler exit (the previous Drop-guard approach):
/// `remote.send(req).await` resolves when the request is accepted by the
/// SDK's mpsc channel, which is *before* the SDK's node loop has dequeued
/// the request and called `File::open(&path)`. An immediate unlink would
/// race the SDK's open, causing intermittent ENOENT failures whose only
/// observable symptom is a silent transfer drop.
///
/// Spawning a long-delay cleanup decouples deletion from the handler
/// lifetime entirely. By the time the delay elapses, the SDK has either
/// (a) opened the file and now holds an FD that survives unlink on POSIX,
/// or (b) failed to consume the request inside 10 minutes - in which case
/// the transfer was already dead and reclaiming the disk is the correct
/// action.
///
/// We remove the per-request directory rather than just the file so a
/// stray subdirectory doesn't leak even on partial cleanup failure.
fn schedule_temp_dir_cleanup(dir: PathBuf) {
    tokio::spawn(async move {
        tokio::time::sleep(TEMP_FILE_TTL).await;
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => {
                info!(target: "citadel", "Cleaned up browser temp dir {:?}", dir);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Already gone (operator sweep, prior cleanup) - fine.
            }
            Err(e) => {
                warn!(
                    target: "citadel",
                    "Failed to clean up browser temp dir {:?}: {}",
                    dir, e
                );
            }
        }
    });
}

/// Materialise raw byte contents into a one-shot temp file and schedule its
/// cleanup. Returns the path for handing to the SDK.
///
/// All disk I/O happens inside `spawn_blocking` so the tokio worker servicing
/// this handler is not parked on a blocking write. Cleanup is scheduled via
/// `schedule_temp_dir_cleanup` *after* the write attempt completes (success
/// OR failure), so partial states like "directory created but write failed"
/// do not leak the empty subdir.
async fn materialize_byte_contents(
    file_name: &str,
    data: Vec<u8>,
) -> Result<PathBuf, NetworkError> {
    let safe_name = sanitize_file_name(file_name);

    // Each request gets its own UUID-named subdirectory under the shared
    // browser-transfer root, with the user-provided file name preserved
    // inside. The SDK reads the *basename* of the path as the transfer's
    // visible filename, so the receiver sees the original `file_name`
    // rather than a UUID-mangled stem. Per-request isolation also means
    // two simultaneous uploads with the same name cannot collide.
    let request_dir = std::env::temp_dir()
        .join(BROWSER_TRANSFER_SUBDIR)
        .join(Uuid::new_v4().to_string());
    let temp_path = request_dir.join(&safe_name);

    let request_dir_for_blocking = request_dir.clone();
    let temp_path_for_blocking = temp_path.clone();
    let bytes_len = data.len();

    let write_result = tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&request_dir_for_blocking)?;
        std::fs::write(&temp_path_for_blocking, &data)
    })
    .await;

    match write_result {
        Ok(Ok(())) => {
            info!(
                target: "citadel",
                "Wrote browser file {:?} ({} bytes) to {:?}",
                safe_name, bytes_len, temp_path
            );
            // Schedule cleanup of the *directory* rather than the file
            // alone, so the request dir doesn't outlive the file.
            schedule_temp_dir_cleanup(request_dir);
            Ok(temp_path)
        }
        Ok(Err(e)) => {
            // The directory may or may not exist depending on which step
            // failed (create_dir_all vs write). Schedule cleanup either
            // way so an empty subdir from a partial-success state cannot
            // leak; remove_dir_all tolerates missing paths.
            schedule_temp_dir_cleanup(request_dir);
            Err(NetworkError::msg(format!(
                "Failed to write browser file to temp: {e}"
            )))
        }
        Err(join_err) => {
            // spawn_blocking JoinError - the closure didn't run to
            // completion (panic or runtime cancellation). The directory
            // may have been partially constructed. Best-effort cleanup.
            schedule_temp_dir_cleanup(request_dir);
            Err(NetworkError::msg(format!(
                "Failed to run blocking temp-file write: {join_err}"
            )))
        }
    }
}

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::SendFile {
        request_id,
        source,
        cid,
        peer_cid,
        chunk_size,
        transfer_type,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };
    let remote = this.remote().clone();

    // Resolve the source to a filesystem path. For ByteContents, this also
    // schedules the temp-file cleanup (see `materialize_byte_contents`).
    //
    // PickFileRef is the only branch that needs the connection-map lock
    // here; ByteContents materialisation deliberately does its disk I/O
    // OUTSIDE any lock so concurrent connection-map writers are not
    // stalled by the spawn_blocking write.
    let resolved_path: Result<PathBuf, NetworkError> = match source {
        FileSource::Path(path) => Ok(path),
        FileSource::PickFileRef {
            pick_file_request_id,
        } => {
            let lock = this.server_connection_map.read();
            match lock.get(&cid) {
                Some(conn) => match conn.picked_files.get(&pick_file_request_id) {
                    Some(picked_info) => {
                        info!(target: "citadel", "Resolved PickFileRef {:?} to path {:?}",
                            pick_file_request_id, picked_info.file_path);
                        Ok(picked_info.file_path.clone())
                    }
                    None => Err(NetworkError::msg(format!(
                        "PickFile reference not found: {:?}. The file picker result may have expired.",
                        pick_file_request_id
                    ))),
                },
                None => Err(NetworkError::msg("Connection not found for PickFileRef lookup")),
            }
        }
        FileSource::ByteContents { file_name, data } => {
            // Size guard: fail fast before any I/O.
            if data.len() > MAX_BYTE_CONTENTS_BYTES {
                Err(NetworkError::msg(format!(
                    "ByteContents payload of {} bytes exceeds the {} byte maximum",
                    data.len(),
                    MAX_BYTE_CONTENTS_BYTES
                )))
            } else {
                materialize_byte_contents(&file_name, data).await
            }
        }
    };

    // Build the NodeRequest under a brief read lock, then drop the lock
    // before any await (the SDK `remote.send` below is async and must not
    // happen while the RwLock is held).
    let send_request: Result<NodeRequest, NetworkError> = match resolved_path {
        Ok(file_path) => {
            let lock = this.server_connection_map.read();
            match lock.get(&cid) {
                Some(conn) => {
                    if let Some(peer_cid) = peer_cid {
                        if conn.peers.contains_key(&peer_cid) {
                            // Use the deterministic `LocalGroupPeer { session_cid,
                            // peer_cid }` value the SDK expects, instead of
                            // requiring `peer_conn.remote.user()`. The previous
                            // implementation gated on `peer_conn.remote` being
                            // `Some`, which is only true on the side that called
                            // `connect_to_peer_custom`. For acceptor-side
                            // connections `remote` is `None` and the request
                            // failed with "Peer connection missing remote
                            // (acceptor-only connection cannot send files)" —
                            // which silently broke file transfer in either
                            // direction whenever the sender wasn't also the
                            // P2P initiator. Same shape we already use for
                            // messaging via the sink works for SendObject too.
                            Ok(NodeRequest::SendObject(SendObject {
                                source: Box::new(file_path),
                                chunk_size,
                                session_cid: cid,
                                v_conn_type: VirtualTargetType::LocalGroupPeer {
                                    session_cid: cid,
                                    peer_cid,
                                },
                                transfer_type,
                            }))
                        } else {
                            Err(NetworkError::msg("Peer Connection Not Found"))
                        }
                    } else {
                        Ok(NodeRequest::SendObject(SendObject {
                            source: Box::new(file_path),
                            chunk_size,
                            session_cid: cid,
                            v_conn_type: VirtualTargetType::LocalGroupServer { session_cid: cid },
                            transfer_type,
                        }))
                    }
                }
                None => {
                    error!(target: "citadel","upload: server connection not found");
                    Err(NetworkError::msg("upload: Server Connection Not Found"))
                }
            }
        }
        Err(e) => Err(e),
    }; // Lock dropped here - BEFORE any await

    match send_request {
        Ok(request) => {
            let result = remote.send(request).await;
            match result {
                Ok(_) => {
                    info!(target: "citadel","InternalServiceRequest Send File Success");
                    let response =
                        InternalServiceResponse::SendFileRequestSuccess(SendFileRequestSuccess {
                            cid,
                            request_id: Some(request_id),
                        });
                    Some(HandledRequestResult { response, uuid })
                }
                Err(err) => {
                    error!(target: "citadel","InternalServiceRequest Send File Failure");
                    let response =
                        InternalServiceResponse::SendFileRequestFailure(SendFileRequestFailure {
                            cid,
                            message: err.into_string(),
                            request_id: Some(request_id),
                        });
                    Some(HandledRequestResult { response, uuid })
                }
            }
        }
        Err(err) => {
            let response =
                InternalServiceResponse::SendFileRequestFailure(SendFileRequestFailure {
                    cid,
                    message: err.into_string(),
                    request_id: Some(request_id),
                });
            Some(HandledRequestResult { response, uuid })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{materialize_byte_contents, sanitize_file_name};

    #[test]
    fn sanitize_strips_path_components() {
        assert_eq!(sanitize_file_name("photos/vacation.jpg"), "vacation.jpg");
        assert_eq!(sanitize_file_name("../../../etc/passwd"), "passwd");
        assert_eq!(sanitize_file_name("plain.txt"), "plain.txt");
    }

    #[test]
    fn sanitize_handles_pathological_input() {
        assert_eq!(sanitize_file_name(""), "upload");
        assert_eq!(sanitize_file_name("/"), "upload");
        // A pure directory-style name has no file component
        assert_eq!(sanitize_file_name("a/b/"), "b");
    }

    /// Exercises the IO-error branch of `materialize_byte_contents`. We
    /// trigger a real `std::fs::write` failure by passing a filename longer
    /// than `NAME_MAX` (typically 255 bytes on ext4 / APFS / NTFS), which
    /// makes the filesystem return ENAMETOOLONG when we try to create the
    /// file inside the per-request subdir.
    ///
    /// This proves that the `Ok(Err(e))` arm is reached (and therefore
    /// schedule_temp_dir_cleanup is invoked - code coverage is the
    /// proof; verifying the deferred cleanup task ran would require
    /// advancing tokio's paused clock 10 minutes, which adds complexity
    /// without strengthening this contract).
    #[tokio::test]
    async fn materialize_returns_err_when_write_fails() {
        // 300 bytes well exceeds NAME_MAX on every mainstream filesystem.
        let overlong = "a".repeat(300);
        let result = materialize_byte_contents(&overlong, vec![1, 2, 3]).await;
        assert!(
            result.is_err(),
            "expected IO error for overlong filename, got Ok({:?})",
            result.ok()
        );
        let msg = result.unwrap_err().into_string();
        assert!(
            msg.contains("Failed to write browser file to temp"),
            "unexpected error message: {msg}"
        );
    }

    /// Happy-path counterpart to `materialize_returns_err_when_write_fails`.
    /// A reasonable filename and small payload must produce a path inside
    /// the configured browser-transfer subdir, with the file actually
    /// present on disk and containing the bytes we asked for.
    #[tokio::test]
    async fn materialize_writes_payload_to_temp_path() {
        let path = materialize_byte_contents("hello.bin", vec![0xDE, 0xAD, 0xBE, 0xEF])
            .await
            .expect("materialize should succeed");

        // Path lives under our isolated subdir.
        let parent_components: Vec<_> = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        assert!(
            parent_components
                .iter()
                .any(|c| c == super::BROWSER_TRANSFER_SUBDIR),
            "path {:?} not under {:?}",
            path,
            super::BROWSER_TRANSFER_SUBDIR
        );
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("hello.bin"));

        // File on disk has exactly the bytes we wrote.
        let read_back = std::fs::read(&path).expect("read written temp file");
        assert_eq!(read_back, vec![0xDE, 0xAD, 0xBE, 0xEF]);

        // Best-effort eager cleanup so the test doesn't lean on the
        // 10-minute deferred TTL. Cleanup of the parent dir is what
        // production relies on; we replicate that here.
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    /// Helper: build an isolated sweep root under `$TMPDIR` so each
    /// test owns its directory and parallel cargo-test threads can't
    /// step on each other. Returns `(root_path, _cleanup_guard)` —
    /// when the guard drops the directory is best-effort removed.
    fn isolated_sweep_root(label: &str) -> (std::path::PathBuf, IsolatedRootGuard) {
        let root = std::env::temp_dir().join(format!(
            "citadel-browser-transfers-test-{}-{}",
            label,
            uuid::Uuid::new_v4()
        ));
        (root.clone(), IsolatedRootGuard(root))
    }

    struct IsolatedRootGuard(std::path::PathBuf);
    impl Drop for IsolatedRootGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Fresh request dirs younger than `max_age` must survive a sweep,
    /// otherwise the sweep would race in-flight uploads.
    #[test]
    fn sweep_preserves_fresh_request_dirs() {
        use super::sweep_browser_transfers_in;
        use std::time::Duration;

        let (root, _guard) = isolated_sweep_root("preserves_fresh");
        std::fs::create_dir_all(&root).expect("create isolated sweep root");

        let fresh = root.join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&fresh).expect("create fresh request dir");
        std::fs::write(fresh.join("payload.bin"), b"fresh").expect("seed fresh payload");

        // A 60-minute threshold ensures the just-created dir is well
        // under the staleness cutoff regardless of CI clock skew.
        sweep_browser_transfers_in(&root, Duration::from_secs(3600));

        assert!(
            fresh.exists(),
            "sweep removed a fresh request dir it should have kept: {fresh:?}"
        );
    }

    /// Stale request dirs (older than `max_age`) must be removed —
    /// this is the actual job of the sweep. Deterministic because
    /// the threshold is `Duration::ZERO`, so everything counts as stale.
    #[test]
    fn sweep_removes_stale_request_dirs() {
        use super::sweep_browser_transfers_in;
        use std::time::Duration;

        let (root, _guard) = isolated_sweep_root("removes_stale");
        std::fs::create_dir_all(&root).expect("create isolated sweep root");

        let stale = root.join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&stale).expect("create stale request dir");
        std::fs::write(stale.join("payload.bin"), b"stale").expect("seed stale payload");

        // `Duration::ZERO` makes every entry exceed the threshold
        // immediately, so the sweep MUST remove the dir.
        sweep_browser_transfers_in(&root, Duration::ZERO);

        assert!(
            !stale.exists(),
            "sweep failed to remove a stale request dir: {stale:?}"
        );
    }

    /// Missing root is the first-boot state. Sweep must not panic or
    /// create the directory itself, even when called against an
    /// absent path. Deterministic because the isolated root is
    /// guaranteed absent (uuid).
    #[test]
    fn sweep_is_a_noop_when_root_is_absent() {
        use super::sweep_browser_transfers_in;
        use std::time::Duration;

        let (root, _guard) = isolated_sweep_root("absent_root");
        // Deliberately do NOT create `root` — it's the missing-root
        // case we want to exercise.
        assert!(!root.exists(), "isolated root unexpectedly already exists");

        sweep_browser_transfers_in(&root, Duration::from_secs(3600));

        assert!(
            !root.exists(),
            "sweep should not create the root dir on its own: {root:?}"
        );
    }
}

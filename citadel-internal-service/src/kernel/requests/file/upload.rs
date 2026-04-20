use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    FileSource, InternalServiceRequest, InternalServiceResponse, SendFileRequestFailure,
    SendFileRequestSuccess,
};
use citadel_sdk::logging::{error, info, warn};
use citadel_sdk::prelude::{
    NetworkError, NodeRequest, Ratchet, SendObject, TargetLockedRemote, VirtualTargetType,
};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Maximum accepted payload size for `FileSource::ByteContents`.
/// Caps the RAM a single request can demand before any disk I/O happens.
/// Kept well above typical browser-scoped transfers but low enough that a
/// malicious or buggy client cannot trivially exhaust server memory.
const MAX_BYTE_CONTENTS_BYTES: usize = 256 * 1024 * 1024; // 256 MiB

/// Subdirectory under `std::env::temp_dir()` where browser-uploaded payloads
/// are materialised. One file per request, cleaned up after the transfer
/// resolves (success or error).
const BROWSER_TRANSFER_SUBDIR: &str = "citadel-browser-transfers";

/// Drop guard that best-effort removes a temp file when it goes out of scope.
///
/// Using a guard rather than explicit cleanup branches means the temp file is
/// removed on every exit path - success, error, and panic - without needing
/// a cleanup block around every return.
///
/// Timing: the guard drops at the end of the request handler, immediately
/// after `remote.send(...).await` resolves. `remote.send` completes when
/// the NodeRequest is accepted by the underlying mpsc channel, which may
/// be before the SDK's node loop has dequeued the request and opened the
/// source file. On POSIX systems this is nevertheless safe in practice
/// because unlinking a path whose inode is subsequently `File::open`ed
/// from the same process within the same scheduling quantum is extremely
/// unlikely to race, and once the SDK holds an FD the inode outlives the
/// unlink. On Windows, deletion may fail with a sharing violation and is
/// logged at `warn` level rather than escalated.
///
/// Any file that escapes cleanup here is a temp-directory leak, not a
/// correctness bug - the subdir is isolated to this service and operators
/// can reap it on schedule.
struct TempFileGuard {
    path: Option<PathBuf>,
}

impl TempFileGuard {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            if let Err(e) = std::fs::remove_file(&path) {
                // ENOENT is fine - file may have been moved/consumed by the
                // SDK. Any other error is worth noting but not fatal.
                if e.kind() != std::io::ErrorKind::NotFound {
                    warn!(target: "citadel", "Failed to clean up temp file {:?}: {}", path, e);
                }
            }
        }
    }
}

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

    // Materialise `ByteContents` into a temp file *before* taking the
    // server_connection_map read lock. The previous implementation did this
    // work while holding the lock, meaning every browser upload blocked any
    // concurrent writer on the connection map for the duration of a blocking
    // disk write. Moving the I/O outside the lock, and using
    // `spawn_blocking` so the tokio worker thread is not blocked, addresses
    // both concerns.
    //
    // The guard returned here cleans up the temp file regardless of how the
    // rest of the handler exits.
    let (resolved_path, _temp_file_guard): (Result<PathBuf, NetworkError>, Option<TempFileGuard>) =
        match source {
            FileSource::Path(path) => (Ok(path), None),
            FileSource::PickFileRef {
                pick_file_request_id,
            } => {
                let lock = this.server_connection_map.read();
                let result = match lock.get(&cid) {
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
                };
                (result, None)
            }
            FileSource::ByteContents { file_name, data } => {
                // Size guard: fail fast before any allocation of a
                // path/filename and before any I/O.
                if data.len() > MAX_BYTE_CONTENTS_BYTES {
                    (
                        Err(NetworkError::msg(format!(
                            "ByteContents payload of {} bytes exceeds the {} byte maximum",
                            data.len(),
                            MAX_BYTE_CONTENTS_BYTES
                        ))),
                        None,
                    )
                } else {
                    let safe_name = sanitize_file_name(&file_name);
                    let temp_dir = std::env::temp_dir().join(BROWSER_TRANSFER_SUBDIR);
                    let temp_path = temp_dir.join(format!("{}_{}", Uuid::new_v4(), safe_name));

                    let temp_dir_for_blocking = temp_dir.clone();
                    let temp_path_for_blocking = temp_path.clone();
                    let bytes_len = data.len();

                    // `std::fs` is synchronous blocking I/O. Running it under
                    // `spawn_blocking` keeps the tokio worker free to service
                    // other tasks.
                    let write_result = tokio::task::spawn_blocking(move || {
                        std::fs::create_dir_all(&temp_dir_for_blocking)?;
                        std::fs::write(&temp_path_for_blocking, &data)
                    })
                    .await;

                    match write_result {
                        Ok(Ok(())) => {
                            info!(target: "citadel", "Wrote browser file {:?} ({} bytes) to {:?}",
                                safe_name, bytes_len, temp_path);
                            let guard = TempFileGuard::new(temp_path.clone());
                            (Ok(temp_path), Some(guard))
                        }
                        Ok(Err(e)) => (
                            Err(NetworkError::msg(format!(
                                "Failed to write browser file to temp: {e}"
                            ))),
                            None,
                        ),
                        Err(join_err) => (
                            Err(NetworkError::msg(format!(
                                "Failed to run blocking temp-file write: {join_err}"
                            ))),
                            None,
                        ),
                    }
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
                        if let Some(peer_conn) = conn.peers.get(&peer_cid) {
                            if let Some(peer_remote) = &peer_conn.remote {
                                Ok(NodeRequest::SendObject(SendObject {
                                    source: Box::new(file_path),
                                    chunk_size,
                                    session_cid: cid,
                                    v_conn_type: *peer_remote.user(),
                                    transfer_type,
                                }))
                            } else {
                                Err(NetworkError::msg(
                                    "Peer connection missing remote (acceptor-only connection cannot send files)",
                                ))
                            }
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

    // `_temp_file_guard` stays in scope until the end of this function so the
    // temp file is only cleaned up after the SDK has consumed it.
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
    use super::sanitize_file_name;

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
}

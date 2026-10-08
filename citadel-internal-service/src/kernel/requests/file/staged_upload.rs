//! `StageUploadChunk`: writing one chunk of a browser file to its staging copy.
//!
//! The decisions are kernel/staged_uploads.rs; this is the disk and the session
//! table. The first chunk reserves the whole file against the browser-transfer
//! root's cap and creates a private request directory (0700) and file (0600) under
//! it; later chunks append. The staged copy is removed when the transfer that sends
//! it ends (browser_payload.rs), by the TTL timer if it is never sent, and its
//! table entry is pruned after the same TTL.

use super::upload::{
    browser_transfer_root_bytes, create_private_dir_exclusive, ensure_private_root,
    sanitize_file_name, schedule_temp_dir_cleanup, MAX_BROWSER_TRANSFER_TOTAL_BYTES, TEMP_FILE_TTL,
};
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::staged_uploads::{plan_chunk, ChunkPlan, Reservation, StagedUpload};
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, StageUploadChunkFailure,
    StageUploadChunkSuccess,
};
use citadel_sdk::prelude::Ratchet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Instant;
use uuid::Uuid;

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::StageUploadChunk {
        request_id,
        cid,
        upload_id,
        file_name,
        total_size,
        offset,
        data,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };
    let outcome = stage(this, cid, upload_id, &file_name, total_size, offset, data).await;
    let response = match outcome {
        Ok(received) => InternalServiceResponse::StageUploadChunkSuccess(StageUploadChunkSuccess {
            cid,
            upload_id,
            received,
            request_id: Some(request_id),
        }),
        Err(message) => InternalServiceResponse::StageUploadChunkFailure(StageUploadChunkFailure {
            cid,
            upload_id,
            message,
            request_id: Some(request_id),
        }),
    };
    Some(HandledRequestResult { response, uuid })
}

async fn stage<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    upload_id: Uuid,
    file_name: &str,
    total: u64,
    offset: u64,
    data: Vec<u8>,
) -> Result<u64, String> {
    let plan = {
        let mut lock = this.server_connection_map.write();
        let conn = lock
            .get_mut(&cid)
            .ok_or_else(|| "stage: Server Connection Not Found".to_string())?;
        for dir in conn.staged_uploads.prune(Instant::now(), TEMP_FILE_TTL) {
            super::browser_payload::remove_request_dir_soon(dir);
        }
        let plan = plan_chunk(
            conn.staged_uploads.get(&upload_id),
            offset,
            data.len(),
            total,
        );
        if plan == ChunkPlan::Append {
            let upload = conn.staged_uploads.get_mut(&upload_id).expect("planned");
            upload.writing = true;
            Some(upload.path.clone())
        } else if let ChunkPlan::Refuse(reason) = plan {
            return Err(reason);
        } else {
            None
        }
    };
    let len = data.len() as u64;
    match plan {
        None => start(this, cid, upload_id, file_name, total, data).await,
        Some(path) => {
            let written = tokio::task::spawn_blocking(move || append(&path, &data)).await;
            let mut lock = this.server_connection_map.write();
            let uploads = &mut lock
                .get_mut(&cid)
                .ok_or_else(|| "stage: Server Connection Not Found".to_string())?
                .staged_uploads;
            let failure = match written {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            if let Some(e) = failure {
                // A half-written chunk leaves the copy unusable: drop it whole,
                // and the browser starts the file again.
                if let Some(upload) = uploads.remove(&upload_id) {
                    super::browser_payload::remove_request_dir_soon(upload.dir);
                }
                return Err(format!("could not write the chunk: {e}"));
            }
            let upload = uploads
                .get_mut(&upload_id)
                .ok_or_else(|| "the staged upload was abandoned".to_string())?;
            upload.writing = false;
            upload.received += len;
            upload.reservation.landed(len);
            Ok(upload.received)
        }
    }
}

async fn start<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    upload_id: Uuid,
    file_name: &str,
    total: u64,
    data: Vec<u8>,
) -> Result<u64, String> {
    let root = this.browser_transfers.path().to_path_buf();
    let in_flight = this.browser_transfers.in_flight().clone();
    let dir = root.join(Uuid::new_v4().to_string());
    let path = dir.join(sanitize_file_name(file_name));
    let (dir_w, path_w) = (dir.clone(), path.clone());
    let len = data.len() as u64;
    let created = tokio::task::spawn_blocking(move || -> Result<Reservation, String> {
        ensure_private_root(&root).map_err(|e| e.to_string())?;
        // The whole file is reserved up front, so concurrent uploads cannot all
        // see the same free space; each chunk that lands moves its bytes from the
        // reservation to the disk.
        let before = in_flight.fetch_add(total, Ordering::SeqCst);
        let mut reservation = Reservation::new(in_flight, total);
        let on_disk = browser_transfer_root_bytes(&root);
        if on_disk.saturating_add(before).saturating_add(total) > MAX_BROWSER_TRANSFER_TOTAL_BYTES {
            return Err(format!(
                "the agent is already holding {} bytes of browser files being sent; this {total} \
                 byte file would pass the {MAX_BROWSER_TRANSFER_TOTAL_BYTES} byte limit. Try again \
                 when those sends finish.",
                on_disk.saturating_add(before)
            ));
        }
        create_private_dir_exclusive(&dir_w).map_err(|e| e.to_string())?;
        create_private_file(&path_w, &data).map_err(|e| e.to_string())?;
        reservation.landed(len);
        Ok(reservation)
    })
    .await
    .map_err(|e| e.to_string())?;
    schedule_temp_dir_cleanup(dir.clone());
    let reservation = match created {
        Ok(reservation) => reservation,
        Err(reason) => {
            super::browser_payload::remove_request_dir_soon(dir);
            return Err(reason);
        }
    };
    let upload = StagedUpload {
        dir: dir.clone(),
        path,
        total,
        received: len,
        writing: false,
        started: Instant::now(),
        reservation,
    };
    let inserted = this
        .server_connection_map
        .write()
        .get_mut(&cid)
        .map(|conn| conn.staged_uploads.insert(upload_id, upload).is_ok());
    if inserted != Some(true) {
        super::browser_payload::remove_request_dir_soon(dir);
        return Err("that upload id is already in use on this session".into());
    }
    Ok(len)
}

fn create_private_file(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(data)
}

fn append(path: &PathBuf, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)?
        .write_all(data)
}

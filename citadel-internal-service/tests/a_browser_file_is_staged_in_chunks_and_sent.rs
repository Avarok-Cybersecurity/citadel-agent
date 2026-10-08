//! A browser file is staged on the agent in acknowledged chunks, then sent whole.
//!
//! A browser send carried the whole file in one `SendFile { ByteContents }`, capped at
//! 16 MiB. `StageUploadChunk` moves it in pieces into a private staging copy, and
//! `SendFile { StagedUpload }` sends that copy over the same Citadel file transfer.

use citadel_internal_service_test_common as common;

use citadel_internal_service_types::{
    FileSource, FileTransferRequestNotification, InternalServiceRequest, InternalServiceResponse,
    StageUploadChunkFailure, StageUploadChunkSuccess,
};
use citadel_sdk::prelude::*;
use common::{
    exhaust_stream_to_file_completion, get_free_port, register_and_connect_to_server_then_peers,
    PeerReturnHandle,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

const CHUNK: usize = 1024 * 1024;

async fn two_peers() -> Vec<PeerReturnHandle> {
    let addrs: Vec<SocketAddr> = (0..2)
        .map(|_| format!("127.0.0.1:{}", get_free_port()).parse().unwrap())
        .collect();
    register_and_connect_to_server_then_peers::<StackedRatchet>(addrs, None, None)
        .await
        .unwrap()
}

/// Stages one chunk and returns the agent's answer to it.
async fn stage(
    to: &UnboundedSender<InternalServiceRequest>,
    from: &mut UnboundedReceiver<InternalServiceResponse>,
    cid: u64,
    upload_id: Uuid,
    total: usize,
    offset: usize,
    data: &[u8],
) -> Result<u64, String> {
    let request_id = Uuid::new_v4();
    to.send(InternalServiceRequest::StageUploadChunk {
        request_id,
        cid,
        upload_id,
        file_name: "staged.bin".to_string(),
        total_size: total as u64,
        offset: offset as u64,
        data: data.to_vec(),
    })
    .unwrap();
    loop {
        match from.recv().await.unwrap() {
            InternalServiceResponse::StageUploadChunkSuccess(StageUploadChunkSuccess {
                received,
                request_id: Some(id),
                ..
            }) if id == request_id => return Ok(received),
            InternalServiceResponse::StageUploadChunkFailure(StageUploadChunkFailure {
                message,
                request_id: Some(id),
                ..
            }) if id == request_id => return Err(message),
            _ => continue,
        }
    }
}

fn payload() -> (Vec<u8>, PathBuf, tempfile::TempDir) {
    let bytes: Vec<u8> = (0..(2 * CHUNK + CHUNK / 2))
        .map(|i| (i * 31 % 251) as u8)
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("staged.bin");
    std::fs::write(&path, &bytes).unwrap();
    (bytes, path, dir)
}

#[tokio::test]
async fn a_file_staged_in_chunks_reaches_the_peer_whole() {
    common::setup_log();
    let mut peers = two_peers().await;
    let (one, two) = peers.split_at_mut(1);
    let (to_a, from_a, cid_a) = &mut one[0];
    let (to_b, from_b, cid_b) = &mut two[0];
    let (bytes, cmp_path, _dir) = payload();
    let upload_id = Uuid::new_v4();

    for (i, chunk) in bytes.chunks(CHUNK).enumerate() {
        let received = stage(
            to_a,
            from_a,
            *cid_a,
            upload_id,
            bytes.len(),
            i * CHUNK,
            chunk,
        )
        .await;
        assert_eq!(received, Ok((i * CHUNK + chunk.len()) as u64));
    }
    to_a.send(InternalServiceRequest::SendFile {
        request_id: Uuid::new_v4(),
        source: FileSource::StagedUpload { upload_id },
        cid: *cid_a,
        transfer_type: TransferType::FileTransfer,
        peer_cid: Some(*cid_b),
        chunk_size: None,
    })
    .unwrap();
    let metadata = loop {
        if let InternalServiceResponse::FileTransferRequestNotification(
            FileTransferRequestNotification { metadata, .. },
        ) = from_b.recv().await.unwrap()
        {
            break metadata;
        }
    };
    to_b.send(InternalServiceRequest::RespondFileTransfer {
        cid: *cid_b,
        peer_cid: *cid_a,
        object_id: metadata.object_id as _,
        accept: true,
        download_location: None,
        request_id: Uuid::new_v4(),
    })
    .unwrap();
    exhaust_stream_to_file_completion(cmp_path, from_b).await;
}

#[tokio::test]
async fn chunks_land_in_order_and_only_a_complete_file_is_sent() {
    common::setup_log();
    let mut peers = two_peers().await;
    let (one, two) = peers.split_at_mut(1);
    let (to_a, from_a, cid_a) = &mut one[0];
    let cid_b = two[0].2;
    let (bytes, _path, _dir) = payload();
    let upload_id = Uuid::new_v4();

    assert!(
        stage(
            to_a,
            from_a,
            *cid_a,
            upload_id,
            bytes.len(),
            CHUNK,
            &bytes[CHUNK..2 * CHUNK]
        )
        .await
        .is_err(),
        "a chunk for an upload that never started must be refused"
    );
    assert_eq!(
        stage(
            to_a,
            from_a,
            *cid_a,
            upload_id,
            bytes.len(),
            0,
            &bytes[..CHUNK]
        )
        .await,
        Ok(CHUNK as u64)
    );
    assert!(
        stage(
            to_a,
            from_a,
            *cid_a,
            upload_id,
            bytes.len(),
            2 * CHUNK,
            &bytes[2 * CHUNK..]
        )
        .await
        .is_err(),
        "a chunk past a gap must be refused"
    );
    assert!(
        stage(
            to_a,
            from_a,
            *cid_a,
            Uuid::new_v4(),
            3 * 1024 * 1024 * 1024,
            0,
            &bytes[..CHUNK]
        )
        .await
        .is_err(),
        "a file over the ceiling must be refused before anything is written"
    );

    let request_id = Uuid::new_v4();
    to_a.send(InternalServiceRequest::SendFile {
        request_id,
        source: FileSource::StagedUpload { upload_id },
        cid: *cid_a,
        transfer_type: TransferType::FileTransfer,
        peer_cid: Some(cid_b),
        chunk_size: None,
    })
    .unwrap();
    loop {
        match from_a.recv().await.unwrap() {
            InternalServiceResponse::SendFileRequestFailure(f)
                if f.request_id == Some(request_id) =>
            {
                break
            }
            InternalServiceResponse::SendFileRequestSuccess(s)
                if s.request_id == Some(request_id) =>
            {
                panic!("an incomplete staged upload was sent")
            }
            _ => continue,
        }
    }
}

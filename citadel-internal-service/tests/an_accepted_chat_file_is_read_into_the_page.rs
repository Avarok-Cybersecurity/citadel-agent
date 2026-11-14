//! A chat file this session accepted is read into the page, so clicking it
//! downloads the original bytes under the original name.
//!
//! The SDK saves an accepted FileTransfer to the agent's own disk and the page
//! learns only the path (ReceptionBeginning). Nothing recorded that file as
//! readable, so `ReadPulledFileChunk` refused it ("has not been pulled") and the
//! chat could only show a path in the agent's hidden data directory. The page
//! names the file by the path the agent reported, and only the session that
//! accepted it, with the sender still its peer, may read it.

use citadel_internal_service_test_common as common;

use citadel_internal_service_types::{
    FileSource, FileTransferRequestNotification, FileTransferTickNotification,
    InternalServiceRequest, InternalServiceResponse, ReadPulledFileChunkFailure,
    ReadPulledFileChunkSuccess,
};
use citadel_sdk::prelude::*;
use common::{
    get_free_port, register_and_connect_to_server_then_peers_as_browser, PeerReturnHandle,
};
use std::error::Error;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

async fn two_peers() -> Result<Vec<PeerReturnHandle>, Box<dyn Error>> {
    let addrs: Vec<SocketAddr> = (0..2)
        .map(|_| format!("127.0.0.1:{}", get_free_port()).parse().unwrap())
        .collect();
    register_and_connect_to_server_then_peers_as_browser::<StackedRatchet>(addrs).await
}

async fn next(svc: &mut UnboundedReceiver<InternalServiceResponse>) -> InternalServiceResponse {
    tokio::time::timeout(Duration::from_secs(60), svc.recv())
        .await
        .expect("no response within 60 s")
        .expect("the service hung up")
}

/// Ticks until the stream's terminal, returning where a reception saved.
async fn to_terminal(svc: &mut UnboundedReceiver<InternalServiceResponse>) -> Option<PathBuf> {
    let mut saved = None;
    loop {
        if let InternalServiceResponse::FileTransferTickNotification(
            FileTransferTickNotification { status, .. },
        ) = next(svc).await
        {
            match status {
                ObjectTransferStatus::ReceptionBeginning(path, _) => saved = Some(path),
                ObjectTransferStatus::ReceptionComplete
                | ObjectTransferStatus::TransferComplete => return saved,
                ObjectTransferStatus::Fail(err) => panic!("transfer failed: {err}"),
                _ => {}
            }
        }
    }
}

async fn read_chunk(
    to: &UnboundedSender<InternalServiceRequest>,
    from: &mut UnboundedReceiver<InternalServiceResponse>,
    cid: u64,
    peer_cid: Option<u64>,
    path: &Path,
    offset: u64,
) -> Result<(u64, Vec<u8>), String> {
    let id = Uuid::new_v4();
    to.send(InternalServiceRequest::ReadPulledFileChunk {
        request_id: id,
        cid,
        peer_cid,
        virtual_directory: path.to_path_buf(),
        offset,
        max_len: 1024 * 1024,
    })
    .unwrap();
    loop {
        match next(from).await {
            InternalServiceResponse::ReadPulledFileChunkSuccess(ReadPulledFileChunkSuccess {
                request_id: Some(r),
                total_size,
                data,
                ..
            }) if r == id => return Ok((total_size, data)),
            InternalServiceResponse::ReadPulledFileChunkFailure(ReadPulledFileChunkFailure {
                request_id: Some(r),
                message,
                ..
            }) if r == id => return Err(message),
            _ => {}
        }
    }
}

#[tokio::test]
async fn an_accepted_chat_file_is_read_by_the_path_it_was_saved_to() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let mut peers = two_peers().await?;
    let (one, two) = peers.split_at_mut(1);
    let (to_a, from_a, cid_a) = &mut one[0];
    let (to_b, from_b, cid_b) = &mut two[0];
    let (cid_a, cid_b) = (*cid_a, *cid_b);
    let expected: Vec<u8> = b"\x89PNG\r\n\x1a\n not really a picture".to_vec();

    to_a.send(InternalServiceRequest::SendFile {
        request_id: Uuid::new_v4(),
        source: FileSource::ByteContents {
            file_name: "holiday photo.png".to_string(),
            data: expected.clone(),
        },
        cid: cid_a,
        transfer_type: TransferType::FileTransfer,
        peer_cid: Some(cid_b),
        chunk_size: None,
    })?;
    let object_id = loop {
        if let InternalServiceResponse::FileTransferRequestNotification(
            FileTransferRequestNotification { metadata, .. },
        ) = next(from_b).await
        {
            break metadata.object_id;
        }
    };
    to_b.send(InternalServiceRequest::RespondFileTransfer {
        cid: cid_b,
        peer_cid: cid_a,
        object_id,
        accept: true,
        download_location: None,
        request_id: Uuid::new_v4(),
    })?;
    let saved = to_terminal(from_b)
        .await
        .expect("the reception names where it saved");
    to_terminal(from_a).await;
    assert_eq!(
        saved.file_name().and_then(|n| n.to_str()),
        Some("holiday photo.png"),
        "the agent keeps the original name"
    );

    let (total, got) = read_chunk(to_b, from_b, cid_b, Some(cid_a), &saved, 0).await?;
    assert_eq!(total, expected.len() as u64);
    assert_eq!(got, expected, "the page gets the bytes that were sent");

    // Only the receiving session, and only with the sender as the named peer.
    let sender = read_chunk(to_a, from_a, cid_a, Some(cid_b), &saved, 0).await;
    assert!(sender.is_err(), "{sender:?}");
    let unnamed = read_chunk(to_b, from_b, cid_b, None, &saved, 0).await;
    assert!(unnamed.is_err(), "{unnamed:?}");
    Ok(())
}

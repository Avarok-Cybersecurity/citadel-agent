//! A file this session pulled back from RE-VFS is read into the page, in order.
//!
//! `DownloadFile` has the SDK save the file on the agent's disk; the page only
//! learns that it landed. `ReadPulledFileChunk` hands the bytes over, naming the
//! file the way it was pulled -- never by a local path -- and only for a session
//! the connection owns, with the pull's peer still its peer.

use citadel_internal_service_test_common as common;

use citadel_internal_service_types::{
    FileSource, FileTransferTickNotification, InternalServiceRequest, InternalServiceResponse,
    ReadPulledFileChunkFailure, ReadPulledFileChunkSuccess,
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

const TEST_FILE: &str = "../resources/test.txt";

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

/// One chunk: Ok((total_size, offset, data)) or Err(message).
async fn read_chunk(
    to: &UnboundedSender<InternalServiceRequest>,
    from: &mut UnboundedReceiver<InternalServiceResponse>,
    cid: u64,
    peer_cid: Option<u64>,
    path: &Path,
    offset: u64,
    max_len: u32,
) -> Result<(u64, u64, Vec<u8>), String> {
    let id = Uuid::new_v4();
    to.send(InternalServiceRequest::ReadPulledFileChunk {
        request_id: id,
        cid,
        peer_cid,
        virtual_directory: path.to_path_buf(),
        offset,
        max_len,
    })
    .unwrap();
    loop {
        match next(from).await {
            InternalServiceResponse::ReadPulledFileChunkSuccess(ReadPulledFileChunkSuccess {
                request_id: Some(r),
                total_size,
                offset,
                data,
                ..
            }) if r == id => return Ok((total_size, offset, data)),
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
async fn a_pulled_file_is_read_whole_and_only_by_its_owner() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let mut peers = two_peers().await?;
    let (one, two) = peers.split_at_mut(1);
    let (to_a, from_a, cid_a) = &mut one[0];
    let (to_b, from_b, cid_b) = &mut two[0];
    let (cid_a, cid_b) = (*cid_a, *cid_b);
    let virtual_path = PathBuf::from("/shared/test.txt");
    let expected = tokio::fs::read(TEST_FILE).await?;

    to_a.send(InternalServiceRequest::SendFile {
        request_id: Uuid::new_v4(),
        source: FileSource::ByteContents {
            file_name: "test.txt".to_string(),
            data: expected.clone(),
        },
        cid: cid_a,
        transfer_type: TransferType::RemoteEncryptedVirtualFilesystem {
            virtual_path: virtual_path.clone(),
            security_level: Default::default(),
        },
        peer_cid: Some(cid_b),
        chunk_size: None,
    })?;
    to_terminal(from_b).await;
    to_terminal(from_a).await;

    // Before the pull: nothing is recorded, so nothing is read.
    let before = read_chunk(to_a, from_a, cid_a, Some(cid_b), &virtual_path, 0, 64).await;
    assert!(
        matches!(&before, Err(m) if m.contains("pull it first")),
        "{before:?}"
    );

    to_a.send(InternalServiceRequest::DownloadFile {
        virtual_directory: virtual_path.clone(),
        security_level: None,
        delete_on_pull: false,
        cid: cid_a,
        peer_cid: Some(cid_b),
        request_id: Uuid::new_v4(),
    })?;
    to_terminal(from_b).await;
    let saved = to_terminal(from_a)
        .await
        .expect("the pull names where it saved");

    // Read it whole, in small chunks so the order rule is exercised.
    let step: u32 = 3;
    let mut got = Vec::new();
    loop {
        let (total, offset, data) = read_chunk(
            to_a,
            from_a,
            cid_a,
            Some(cid_b),
            &virtual_path,
            got.len() as u64,
            step,
        )
        .await?;
        assert_eq!(total, expected.len() as u64);
        assert_eq!(offset, got.len() as u64);
        assert!(data.len() <= step as usize && !data.is_empty());
        got.extend_from_slice(&data);
        if offset + data.len() as u64 == total {
            break;
        }
    }
    assert_eq!(got, expected, "the page gets the plaintext that was pulled");

    // Past the end, and out of order, are refused.
    let past = read_chunk(
        to_a,
        from_a,
        cid_a,
        Some(cid_b),
        &virtual_path,
        got.len() as u64,
        step,
    )
    .await;
    assert!(past.is_err(), "{past:?}");
    let skipped = read_chunk(to_a, from_a, cid_a, Some(cid_b), &virtual_path, 0, step).await?;
    assert_eq!(skipped.1, 0, "0 restarts");
    let out_of_order = read_chunk(
        to_a,
        from_a,
        cid_a,
        Some(cid_b),
        &virtual_path,
        2 * step as u64,
        step,
    )
    .await;
    assert!(
        matches!(&out_of_order, Err(m) if m.contains("out of order")),
        "{out_of_order:?}"
    );

    // A peer that is not this session's, and the same file without its peer.
    let stranger = read_chunk(
        to_a,
        from_a,
        cid_a,
        Some(cid_b + 1000),
        &virtual_path,
        0,
        step,
    )
    .await;
    assert!(
        matches!(&stranger, Err(m) if m.contains("not a peer")),
        "{stranger:?}"
    );
    let unscoped = read_chunk(to_a, from_a, cid_a, None, &virtual_path, 0, step).await;
    assert!(
        matches!(&unscoped, Err(m) if m.contains("pull it first")),
        "{unscoped:?}"
    );

    // Another connection naming A's session gets nothing.
    let not_owned = read_chunk(to_b, from_b, cid_a, Some(cid_b), &virtual_path, 0, step).await;
    assert!(
        matches!(&not_owned, Err(m) if m.contains("Session unavailable")),
        "{not_owned:?}"
    );

    // Another account on this agent pulls a same-named file from the same
    // source: the SDK writes it over this one. A's read must not return it.
    let mut theirs = expected.clone();
    theirs.reverse();
    std::fs::remove_file(&saved)?;
    std::fs::write(&saved, &theirs)?;
    let replaced = read_chunk(to_a, from_a, cid_a, Some(cid_b), &virtual_path, 0, step).await;
    assert!(
        matches!(&replaced, Err(m) if m.contains("replaced")),
        "{replaced:?}"
    );
    Ok(())
}

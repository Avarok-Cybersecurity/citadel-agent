//! A file this session pulled back from RE-VFS may be sent on to a peer.
//!
//! RE-VFS is pusher-owned: Bob's node holds Alice's upload encrypted for Alice,
//! so Bob can never open it (`a_revfs_push_cannot_be_pulled_by_its_recipient`).
//! For Bob to get a readable copy of a file in their shared storage, Alice's
//! agent has to pull it back and send it to him as an ordinary transfer. A
//! `SendFile` naming a `Path` is refused unless this session picked it, so the
//! one thing the owner could do was refused too. A pull's completed output is
//! a file the agent itself wrote for this session, and is now sendable for the
//! same window as a picked file -- nothing else is.

use citadel_internal_service_test_common as common;

use citadel_internal_service_types::{
    FileSource, FileTransferRequestNotification, FileTransferTickNotification,
    InternalServiceRequest, InternalServiceResponse, SendFileRequestFailure,
    SendFileRequestSuccess,
};
use citadel_sdk::prelude::*;
use common::{
    get_free_port, register_and_connect_to_server_then_peers_as_browser, PeerReturnHandle,
};
use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

const TEST_FILE: &str = "../resources/test.txt";

async fn two_peers() -> Result<Vec<PeerReturnHandle>, Box<dyn Error>> {
    let addrs: Vec<SocketAddr> = (0..2)
        .map(|_| format!("127.0.0.1:{}", get_free_port()).parse().unwrap())
        .collect();
    register_and_connect_to_server_then_peers_as_browser::<StackedRatchet>(addrs).await
}

/// The next response, or a failed test -- never a hung one.
async fn next(svc: &mut UnboundedReceiver<InternalServiceResponse>) -> InternalServiceResponse {
    tokio::time::timeout(Duration::from_secs(60), svc.recv())
        .await
        .expect("no response within 60 s")
        .expect("the service hung up")
}

/// Ticks until the stream's terminal, returning the reception path if any.
async fn to_terminal(svc: &mut UnboundedReceiver<InternalServiceResponse>) -> Option<PathBuf> {
    let mut path = None;
    loop {
        if let InternalServiceResponse::FileTransferTickNotification(
            FileTransferTickNotification { status, .. },
        ) = next(svc).await
        {
            match status {
                ObjectTransferStatus::ReceptionBeginning(p, _) => path = Some(p),
                ObjectTransferStatus::ReceptionComplete
                | ObjectTransferStatus::TransferComplete => return path,
                ObjectTransferStatus::Fail(err) => panic!("transfer failed: {err}"),
                _ => {}
            }
        }
    }
}

/// The SendFile's verdict: Ok on success, Err(message) on refusal.
async fn send_verdict(
    svc: &mut UnboundedReceiver<InternalServiceResponse>,
    id: Uuid,
) -> Result<(), String> {
    loop {
        match next(svc).await {
            InternalServiceResponse::SendFileRequestSuccess(SendFileRequestSuccess {
                request_id: Some(r),
                ..
            }) if r == id => return Ok(()),
            InternalServiceResponse::SendFileRequestFailure(SendFileRequestFailure {
                request_id: Some(r),
                message,
                ..
            }) if r == id => return Err(message),
            _ => {}
        }
    }
}

fn send_path(cid: u64, peer: u64, path: PathBuf, id: Uuid) -> InternalServiceRequest {
    InternalServiceRequest::SendFile {
        request_id: id,
        source: FileSource::Path(path),
        cid,
        transfer_type: TransferType::FileTransfer,
        peer_cid: Some(peer),
        chunk_size: None,
    }
}

#[tokio::test]
async fn a_file_this_session_pulled_can_be_sent_to_the_peer() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let mut peers = two_peers().await?;
    let (one, two) = peers.split_at_mut(1);
    let (to_a, from_a, cid_a) = &mut one[0];
    let (to_b, from_b, cid_b) = &mut two[0];
    let virtual_path = PathBuf::from("/shared/test.txt");

    // ByteContents: what a browser uploads with, and the only source it may name.
    to_a.send(InternalServiceRequest::SendFile {
        request_id: Uuid::new_v4(),
        source: FileSource::ByteContents {
            file_name: "test.txt".to_string(),
            data: tokio::fs::read(TEST_FILE).await?,
        },
        cid: *cid_a,
        transfer_type: TransferType::RemoteEncryptedVirtualFilesystem {
            virtual_path: virtual_path.clone(),
            security_level: Default::default(),
        },
        peer_cid: Some(*cid_b),
        chunk_size: None,
    })?;
    let _ = to_terminal(from_b).await;
    let _ = to_terminal(from_a).await;

    to_a.send(InternalServiceRequest::DownloadFile {
        virtual_directory: virtual_path,
        security_level: None,
        delete_on_pull: false,
        cid: *cid_a,
        peer_cid: Some(*cid_b),
        request_id: Uuid::new_v4(),
    })?;
    let _holder_answered = to_terminal(from_b).await;
    let pulled = to_terminal(from_a)
        .await
        .expect("the pull names where it saved");

    let share_id = Uuid::new_v4();
    to_a.send(send_path(*cid_a, *cid_b, pulled, share_id))?;
    assert_eq!(send_verdict(from_a, share_id).await, Ok(()));

    let metadata = loop {
        if let InternalServiceResponse::FileTransferRequestNotification(
            FileTransferRequestNotification { metadata, .. },
        ) = next(from_b).await
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
    })?;
    let received = to_terminal(from_b).await.expect("B saved it somewhere");
    assert_eq!(
        tokio::fs::read(received).await?,
        tokio::fs::read(TEST_FILE).await?,
        "B must get the plaintext Alice uploaded"
    );
    Ok(())
}

#[tokio::test]
async fn a_path_this_session_never_pulled_or_picked_is_still_refused() -> Result<(), Box<dyn Error>>
{
    common::setup_log();
    let mut peers = two_peers().await?;
    let (one, two) = peers.split_at_mut(1);
    let (to_a, from_a, cid_a) = &mut one[0];
    let (_to_b, _from_b, cid_b) = &mut two[0];
    let id = Uuid::new_v4();
    to_a.send(send_path(*cid_a, *cid_b, PathBuf::from(TEST_FILE), id))?;
    let verdict = send_verdict(from_a, id).await;
    assert!(
        matches!(&verdict, Err(m) if m.contains("file picker")),
        "{verdict:?}"
    );
    Ok(())
}

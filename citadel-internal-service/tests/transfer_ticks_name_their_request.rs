//! A tick stream names the request that started it, or nothing.
//!
//! The browser joins a `FileTransferTickNotification` to a chat transfer by its
//! `request_id`. Sender-side ticks of a peer transfer used to carry the
//! session's localhost TCP-connection uuid instead -- one value shared by every
//! stream the session ever saw, including the reception of a peer's RE-VFS push
//! into this node. The browser marks a RE-VFS reception's id as "not a chat
//! transfer" and drops everything that carries it afterwards, so once a peer had
//! stored a file here, every later send's progress was thrown away: the
//! receiver saved the file and the sender sat at "Sending... 0%" for good.
//!
//! Also pinned here: a RE-VFS push is retrievable only by the node that pushed
//! it. The recipient asking the pusher for it reads the PUSHER's disk, where it
//! does not exist -- which is why a Windows sender's ENOENT text showed up on the
//! Mac that tried to open the "standard" transfer.

use citadel_internal_service_test_common as common;

use citadel_internal_service_types::{
    DownloadFileFailure, FileSource, FileTransferRequestNotification, FileTransferTickNotification,
    InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::prelude::*;
use common::{get_free_port, register_and_connect_to_server_then_peers, PeerReturnHandle};
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
    register_and_connect_to_server_then_peers::<StackedRatchet>(addrs, None, None).await
}

/// Every tick until the stream's terminal status, with its request_id.
async fn ticks_to_terminal(
    svc: &mut UnboundedReceiver<InternalServiceResponse>,
) -> Vec<(ObjectTransferStatus, Option<Uuid>)> {
    let mut seen = Vec::new();
    loop {
        let InternalServiceResponse::FileTransferTickNotification(FileTransferTickNotification {
            status,
            request_id,
            ..
        }) = svc.recv().await.unwrap()
        else {
            continue;
        };
        let terminal = matches!(
            status,
            ObjectTransferStatus::TransferComplete
                | ObjectTransferStatus::ReceptionComplete
                | ObjectTransferStatus::Fail(_)
        );
        seen.push((status, request_id));
        if terminal {
            return seen;
        }
    }
}

async fn send_and_accept_after(
    peers: &mut [PeerReturnHandle],
    accept_after: Duration,
) -> Result<(), Box<dyn Error>> {
    let (one, two) = peers.split_at_mut(1);
    let (to_a, from_a, cid_a) = &mut one[0];
    let (to_b, from_b, cid_b) = &mut two[0];
    let send_id = Uuid::new_v4();
    to_a.send(InternalServiceRequest::SendFile {
        request_id: send_id,
        source: FileSource::Path(PathBuf::from(TEST_FILE)),
        cid: *cid_a,
        transfer_type: TransferType::FileTransfer,
        peer_cid: Some(*cid_b),
        chunk_size: None,
    })?;

    let metadata = loop {
        if let InternalServiceResponse::FileTransferRequestNotification(
            FileTransferRequestNotification { metadata, .. },
        ) = from_b.recv().await.unwrap()
        {
            break metadata;
        }
    };
    tokio::time::sleep(accept_after).await;
    to_b.send(InternalServiceRequest::RespondFileTransfer {
        cid: *cid_b,
        peer_cid: *cid_a,
        object_id: metadata.object_id as _,
        accept: true,
        download_location: None,
        request_id: Uuid::new_v4(),
    })?;

    let sender_ticks = ticks_to_terminal(from_a).await;
    assert!(
        matches!(
            sender_ticks.last(),
            Some((ObjectTransferStatus::TransferComplete, _))
        ),
        "the send must complete: {sender_ticks:?}"
    );
    for (status, request_id) in &sender_ticks {
        assert_eq!(
            *request_id,
            Some(send_id),
            "sender tick {status:?} must name the SendFile that started it"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_peer_send_s_ticks_name_its_send_request() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let mut peers = two_peers().await?;
    send_and_accept_after(&mut peers, Duration::ZERO).await
}

/// A person takes longer than the upload's 30 s first-event wait to click
/// Accept, so the Sender handle reaches the kernel's catch-all handler instead
/// of the request's own subscription. Its ticks must still name the send.
#[tokio::test]
async fn a_send_accepted_after_the_first_event_wait_still_names_its_request(
) -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let mut peers = two_peers().await?;
    send_and_accept_after(&mut peers, Duration::from_secs(32)).await
}

#[tokio::test]
async fn a_revfs_push_s_reception_names_no_request() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let mut peers = two_peers().await?;
    let (one, two) = peers.split_at_mut(1);
    let (to_a, _from_a, cid_a) = &mut one[0];
    let (_to_b, from_b, cid_b) = &mut two[0];
    to_a.send(InternalServiceRequest::SendFile {
        request_id: Uuid::new_v4(),
        source: FileSource::Path(PathBuf::from(TEST_FILE)),
        cid: *cid_a,
        transfer_type: TransferType::RemoteEncryptedVirtualFilesystem {
            virtual_path: PathBuf::from("/vfs/test.txt"),
            security_level: Default::default(),
        },
        peer_cid: Some(*cid_b),
        chunk_size: None,
    })?;
    let holder_ticks = ticks_to_terminal(from_b).await;
    for (status, request_id) in &holder_ticks {
        assert_eq!(
            *request_id, None,
            "B requested nothing, so {status:?} must not carry an id the browser could join"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_revfs_push_cannot_be_pulled_by_its_recipient() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let mut peers = two_peers().await?;
    let (one, two) = peers.split_at_mut(1);
    let (to_a, _from_a, cid_a) = &mut one[0];
    let (to_b, from_b, cid_b) = &mut two[0];
    let virtual_path = PathBuf::from("/transfers/t1/test.txt");
    to_a.send(InternalServiceRequest::SendFile {
        request_id: Uuid::new_v4(),
        source: FileSource::Path(PathBuf::from(TEST_FILE)),
        cid: *cid_a,
        transfer_type: TransferType::RemoteEncryptedVirtualFilesystem {
            virtual_path: virtual_path.clone(),
            security_level: Default::default(),
        },
        peer_cid: Some(*cid_b),
        chunk_size: None,
    })?;
    let _stored_on_b = ticks_to_terminal(from_b).await;

    to_b.send(InternalServiceRequest::DownloadFile {
        virtual_directory: virtual_path,
        security_level: None,
        delete_on_pull: false,
        cid: *cid_b,
        peer_cid: Some(*cid_a),
        request_id: Uuid::new_v4(),
    })?;
    let message = loop {
        if let InternalServiceResponse::DownloadFileFailure(DownloadFileFailure {
            message, ..
        }) = from_b.recv().await.unwrap()
        {
            break message;
        }
    };
    assert!(
        message.contains("os error 2"),
        "the pull reads A's disk, where B's copy is not: {message}"
    );
    Ok(())
}

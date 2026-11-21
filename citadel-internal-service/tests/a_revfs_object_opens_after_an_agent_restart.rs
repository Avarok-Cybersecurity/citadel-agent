//! A RE-VFS object is sealed under the account's key (SA-10) and still opens after the agent
//! restarts: the account's root (its recovery phrase's seed) is stored at registration, sealed
//! in the account's store under the agent's vault key, and the restarted agent derives the same
//! key from it.
//!
//! Controls (each turns this test red): drop `with_revfs_key_source` from the test agent (the
//! push fails `RevfsKeyMissing`); keep the vault in memory across the restart (the sign-in is
//! refused: the stored roots do not open).

#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service_test_common as common;
use citadel_internal_service_types::{
    DownloadFileFailure, FileSource, FileTransferTickNotification, InternalServiceRequest,
    InternalServiceResponse, SendFileRequestFailure,
};
use citadel_sdk::prelude::*;
use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use support::{
    expect, open, register, sign_in_after_restart, spawn_agent, temp_store, username, Stream,
};
use uuid::Uuid;

/// A server that accepts the objects it is sent, on the first address `localhost` resolves to.
async fn spawn_storing_server() -> Result<u16, Box<dyn Error>> {
    let first = tokio::net::lookup_host("localhost:0")
        .await?
        .next()
        .ok_or("localhost resolves to nothing")?;
    let kernel = common::ReceiverFileTransferKernel::<StackedRatchet>(
        None,
        Arc::new(AtomicBool::new(false)),
    );
    let (server, addr) =
        common::server_test_node_on_listener(kernel, std::net::TcpListener::bind(first)?, |_| {});
    tokio::spawn(server);
    Ok(addr.port())
}

/// The transfer's terminal tick: `Ok(path received to)` or the failure.
async fn terminal(stream: &mut Stream) -> Result<Option<PathBuf>, Box<dyn Error>> {
    let mut received = None;
    expect(stream, |response| match response {
        InternalServiceResponse::SendFileRequestFailure(SendFileRequestFailure {
            message, ..
        })
        | InternalServiceResponse::DownloadFileFailure(DownloadFileFailure { message, .. }) => {
            Some(Err(message))
        }
        InternalServiceResponse::FileTransferTickNotification(FileTransferTickNotification {
            status,
            ..
        }) => match status {
            ObjectTransferStatus::ReceptionBeginning(path, _) => {
                received = Some(path);
                None
            }
            ObjectTransferStatus::TransferComplete => Some(Ok(None)),
            ObjectTransferStatus::ReceptionComplete => Some(Ok(received.clone())),
            ObjectTransferStatus::Fail(err) => Some(Err(err)),
            _ => None,
        },
        _ => None,
    })
    .await?
    .map_err(|err| format!("the RE-VFS transfer failed: {err}").into())
}

#[tokio::test]
async fn a_revfs_object_opens_after_an_agent_restart() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let port = spawn_storing_server().await?;
    let store = temp_store();
    let typed = format!("localhost:{port}");
    let name = username();
    let original = PathBuf::from("../resources/test.txt");
    let virtual_path = PathBuf::from("/vfs/restart.txt");

    let (agent, node) = spawn_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;
    let cid = register(&mut sink, &mut stream, &typed, &name).await??;
    common::send(
        &mut sink,
        InternalServiceRequest::SendFile {
            request_id: Uuid::new_v4(),
            source: FileSource::Path(original.clone()),
            cid,
            transfer_type: TransferType::RemoteEncryptedVirtualFilesystem {
                virtual_path: virtual_path.clone(),
                security_level: Default::default(),
            },
            peer_cid: None,
            chunk_size: None,
        },
    )
    .await?;
    assert_eq!(terminal(&mut stream).await?, None, "the push completes");

    common::send(
        &mut sink,
        InternalServiceRequest::Disconnect {
            request_id: Uuid::new_v4(),
            cid,
        },
    )
    .await?;
    expect(&mut stream, |response| match response {
        InternalServiceResponse::DisconnectNotification(_) => Some(()),
        _ => None,
    })
    .await?;
    drop((sink, stream));
    node.abort();
    let _ = node.await;

    let (agent, _node) = spawn_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;
    let cid = sign_in_after_restart(&mut sink, &mut stream, &typed, &name).await?;
    common::send(
        &mut sink,
        InternalServiceRequest::DownloadFile {
            virtual_directory: virtual_path,
            security_level: None,
            delete_on_pull: false,
            cid,
            peer_cid: None,
            request_id: Uuid::new_v4(),
        },
    )
    .await?;
    let pulled = terminal(&mut stream)
        .await?
        .ok_or("the pull named no file")?;
    assert_eq!(
        tokio::fs::read(&pulled).await?,
        tokio::fs::read(&original).await?,
        "the object pulled after the restart is the one pushed before it"
    );
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

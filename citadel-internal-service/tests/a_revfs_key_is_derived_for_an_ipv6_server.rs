//! A server reached by an IPv6 literal gets a RE-VFS key, and a server address with no
//! canonical domain still refuses the sign-in (owner, 2026-11-19).
//!
//! The account's RE-VFS key is derived over its server's canonical domain. `canonical_domain`
//! split the port at the last `:` and allowed only `[a-z0-9-.]` labels, so `[::1]:<port>` had
//! none: the agent could derive no key and refused every Connect to an IPv6-literal server. A
//! bracketed literal now canonicalises, and the key derivation still fails fast, naming the
//! cause, for an address that has no canonical domain.
//!
//! Controls: send bracketed input down the name path in `citadel-identity`'s `split_authority`
//! (`an_ipv6_literal_server_gets_a_revfs_key` fails at the Connect: "has no canonical domain");
//! in `revfs_seed::session_key`, derive over the address as typed when it has no canonical
//! domain (`a_server_with_no_canonical_domain_refuses_the_sign_in` signs in).

#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service_test_common as common;
use citadel_internal_service_types::{
    FileSource, FileTransferTickNotification, InternalServiceRequest, InternalServiceResponse,
    SendFileRequestFailure,
};
use citadel_sdk::prelude::*;
use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use support::{expect, open, register, spawn_agent, temp_store, username};
use uuid::Uuid;

/// A server that accepts the objects it is sent, listening on `bind`.
fn spawn_storing_server(bind: &str) -> Result<u16, Box<dyn Error>> {
    let kernel = common::ReceiverFileTransferKernel::<StackedRatchet>(
        None,
        Arc::new(AtomicBool::new(false)),
    );
    let (server, addr) =
        common::server_test_node_on_listener(kernel, std::net::TcpListener::bind(bind)?, |_| {});
    tokio::spawn(server);
    Ok(addr.port())
}

#[tokio::test]
async fn an_ipv6_literal_server_gets_a_revfs_key() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let port = spawn_storing_server("[::1]:0")?;
    let store = temp_store();
    let (agent, _node) = spawn_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;
    let cid = register(
        &mut sink,
        &mut stream,
        &format!("[::1]:{port}"),
        &username(),
    )
    .await?
    .map_err(|why| format!("the sign-in to an IPv6-literal server was refused: {why}"))?;

    // Only a session holding a RE-VFS key can push: without one the SDK fails the push.
    common::send(
        &mut sink,
        InternalServiceRequest::SendFile {
            request_id: Uuid::new_v4(),
            source: FileSource::Path(PathBuf::from("../resources/test.txt")),
            cid,
            transfer_type: TransferType::RemoteEncryptedVirtualFilesystem {
                virtual_path: PathBuf::from("/vfs/ipv6.txt"),
                security_level: Default::default(),
            },
            peer_cid: None,
            chunk_size: None,
        },
    )
    .await?;
    let pushed = expect(&mut stream, |response| match response {
        InternalServiceResponse::SendFileRequestFailure(SendFileRequestFailure {
            message, ..
        }) => Some(Err(message)),
        InternalServiceResponse::FileTransferTickNotification(FileTransferTickNotification {
            status,
            ..
        }) => match status {
            ObjectTransferStatus::TransferComplete => Some(Ok(())),
            ObjectTransferStatus::Fail(err) => Some(Err(err)),
            _ => None,
        },
        _ => None,
    })
    .await?;
    assert_eq!(pushed, Ok(()), "the RE-VFS push to an IPv6-literal server");
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

#[tokio::test]
async fn a_server_with_no_canonical_domain_refuses_the_sign_in() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let port = spawn_storing_server("127.0.0.1:0")?;
    let store = temp_store();
    let (agent, _node) = spawn_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;
    // A leading zero dials (the port parses) but is not canonical, so there is no domain to
    // derive the account's key over, and nothing may be stored unsealed.
    let typed = format!("127.0.0.1:0{port}");
    let refused = register(&mut sink, &mut stream, &typed, &username()).await?;
    let why = refused.expect_err("signed in with no RE-VFS key");
    assert!(
        why.contains("storage key is unavailable") && why.contains("has no canonical domain"),
        "the refusal names its cause: {why}"
    );
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

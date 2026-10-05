//! A browser upload's payload leaves the transfer root when its transfer ends.
//!
//! `ByteContents` is written under the service's browser-transfer root and counted
//! against its 256 MiB cap. Only a 10-minute timer (and the startup sweep) removed it:
//! nothing did when the transfer completed or was refused, so browser sends began to
//! fail once about 256 MiB had been sent in any rolling 10 minutes. The cap counts what
//! is on disk under the root, so removing the payload is also what returns its bytes.

use citadel_internal_service_test_common as common;

use citadel_internal_service::kernel::CitadelWorkspaceService;
use citadel_internal_service::BrowserTransferRoot;
use citadel_internal_service_types::{
    FileSource, FileTransferTickNotification, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::prefabs::server::empty::EmptyKernel;
use citadel_sdk::prelude::*;
use common::{
    get_free_port, register_and_connect_to_server, server_info_file_transfer,
    server_test_node_skip_cert_verification, test_backend, RegisterAndConnectItems,
};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// Long enough for the removal task to run, far short of the 10-minute backstop.
const REMOVAL_WAIT: Duration = Duration::from_secs(10);

async fn agent(transfers: BrowserTransferRoot, backend: BackendType) -> SocketAddr {
    let addr: SocketAddr = format!("127.0.0.1:{}", get_free_port()).parse().unwrap();
    let kernel = CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(
        addr,
        citadel_internal_service::SERVER_RECONNECT,
        transfers,
    )
    .await
    .unwrap();
    let node = NodeBuilder::default()
        .with_backend(backend)
        .with_node_type(NodeType::Peer)
        .with_insecure_skip_cert_verification()
        .build(kernel)
        .unwrap();
    tokio::task::spawn(node);
    tokio::time::sleep(Duration::from_millis(2000)).await;
    addr
}

fn payloads_in(root: &Path) -> usize {
    std::fs::read_dir(root).map(|e| e.count()).unwrap_or(0)
}

async fn assert_emptied(root: &Path, when: &str) {
    let gone = tokio::time::timeout(REMOVAL_WAIT, async {
        while payloads_in(root) != 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(
        gone.is_ok(),
        "{} payload(s) still under the root after the transfer {when}",
        payloads_in(root)
    );
}

async fn send_bytes(
    agent: SocketAddr,
    server: SocketAddr,
) -> (
    tokio::sync::mpsc::UnboundedReceiver<InternalServiceResponse>,
    Uuid,
) {
    let (tx, rx, cid) = register_and_connect_to_server(vec![RegisterAndConnectItems {
        internal_service_addr: agent,
        server_addr: server,
        full_name: "payload.user".to_string(),
        username: "payload.user".to_string(),
        password: "secret",
        pre_shared_key: None::<PreSharedKey>,
    }])
    .await
    .unwrap()
    .pop()
    .unwrap();
    let request_id = Uuid::new_v4();
    tx.send(InternalServiceRequest::SendFile {
        request_id,
        source: FileSource::ByteContents {
            // What the file-transfer test server checks the received bytes against.
            file_name: "test.txt".to_string(),
            data: std::fs::read("../resources/test.txt").unwrap(),
        },
        cid,
        transfer_type: TransferType::FileTransfer,
        peer_cid: None,
        chunk_size: None,
    })
    .unwrap();
    // The registration's tx must outlive the transfer, or the session's link closes.
    std::mem::forget(tx);
    (rx, request_id)
}

#[tokio::test]
async fn a_completed_send_removes_its_payload() {
    common::setup_log();
    let (server, server_addr) =
        server_info_file_transfer::<StackedRatchet>(Arc::new(AtomicBool::new(false)));
    tokio::task::spawn(server);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("transfers");
    let agent = agent(
        BrowserTransferRoot::at(root.clone()),
        BackendType::Filesystem(dir.path().join("fs").to_string_lossy().into_owned()),
    )
    .await;

    let (mut rx, _) = send_bytes(agent, server_addr).await;
    loop {
        if let InternalServiceResponse::FileTransferTickNotification(
            FileTransferTickNotification { status, .. },
        ) = rx.recv().await.expect("service stream closed")
        {
            match status {
                ObjectTransferStatus::TransferComplete => break,
                ObjectTransferStatus::Fail(err) => panic!("the send failed: {err}"),
                _ => {}
            }
        }
    }
    assert_emptied(&root, "completed").await;
}

#[tokio::test]
async fn a_refused_send_removes_its_payload() {
    common::setup_log();
    let (server, server_addr) =
        server_test_node_skip_cert_verification(EmptyKernel::<StackedRatchet>::default(), |b| {
            b.with_backend(test_backend());
        });
    tokio::task::spawn(server);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("transfers");
    // In memory: the SDK refuses the transfer AFTER the payload was written.
    let agent = agent(BrowserTransferRoot::at(root.clone()), BackendType::InMemory).await;

    let (mut rx, request_id) = send_bytes(agent, server_addr).await;
    loop {
        if let InternalServiceResponse::SendFileRequestFailure(f) =
            rx.recv().await.expect("service stream closed")
        {
            if f.request_id == Some(request_id) {
                break;
            }
        }
    }
    assert_emptied(&root, "was refused").await;
}

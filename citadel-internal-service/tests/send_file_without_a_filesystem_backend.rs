//! A transfer the SDK refuses must reach the client as a refusal.
//!
//! File transfer needs a filesystem backend on both ends. The agent runs
//! in-memory unless started with `--backend filesystem`, and a tester who forgets
//! the flag sent a file and got `SendFileRequestSuccess` ("queued") -- then
//! nothing, ever. The SDK does refuse, with the right words ("...Both nodes must
//! use a filesystem backend"), but as a node result for the request's ticket,
//! and SendFile used a plain `send`: the refusal fell into the kernel's
//! catch-all and was logged as "Unhandled node result". The UI spun forever.

use citadel_internal_service_test_common as common;

use citadel_internal_service::kernel::CitadelWorkspaceService;
use citadel_internal_service_types::{FileSource, InternalServiceRequest, InternalServiceResponse};
use citadel_sdk::prefabs::server::empty::EmptyKernel;
use citadel_sdk::prelude::*;
use common::{
    connect_p2p, get_free_port, register_and_connect_to_server, register_p2p,
    server_test_node_skip_cert_verification, test_backend, ReceiverFileTransferKernel,
    RegisterAndConnectItems,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

type Service = (
    UnboundedSender<InternalServiceRequest>,
    UnboundedReceiver<InternalServiceResponse>,
    u64,
);

async fn spawn_agent(backend: BackendType) -> SocketAddr {
    let addr: SocketAddr = format!("127.0.0.1:{}", get_free_port()).parse().unwrap();
    let kernel = CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(addr)
        .await
        .unwrap();
    let node = NodeBuilder::default()
        .with_backend(backend)
        .with_node_type(NodeType::Peer)
        .with_insecure_skip_cert_verification()
        .build(kernel)
        .unwrap();
    tokio::task::spawn(node);
    addr
}

async fn login(agent: SocketAddr, server: SocketAddr, user: &str) -> Service {
    register_and_connect_to_server(vec![RegisterAndConnectItems {
        internal_service_addr: agent,
        server_addr: server,
        full_name: user.to_string(),
        username: user.to_string(),
        password: "secret",
        pre_shared_key: None::<PreSharedKey>,
    }])
    .await
    .unwrap()
    .pop()
    .unwrap()
}

fn send_file(cid: u64, peer_cid: Option<u64>) -> (Uuid, InternalServiceRequest) {
    let request_id = Uuid::new_v4();
    let request = InternalServiceRequest::SendFile {
        request_id,
        source: FileSource::Path(PathBuf::from("../resources/test.txt")),
        cid,
        transfer_type: TransferType::FileTransfer,
        peer_cid,
        chunk_size: None,
    };
    (request_id, request)
}

/// The refusal, or a panic saying the client was left waiting.
async fn refusal_for(
    rx: &mut UnboundedReceiver<InternalServiceResponse>,
    request_id: Uuid,
) -> String {
    let found = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match rx.recv().await.expect("service stream closed") {
                InternalServiceResponse::SendFileRequestFailure(f) => return f,
                // "queued": the refusal must still follow it.
                InternalServiceResponse::SendFileRequestSuccess(_) => continue,
                other => panic!("unexpected response while waiting for the refusal: {other:?}"),
            }
        }
    })
    .await
    .expect(
        "SendFile went silent: no SendFileRequestFailure within 20 s, so the client waits forever",
    );
    assert_eq!(
        found.request_id,
        Some(request_id),
        "the refusal names another request"
    );
    found.message
}

#[tokio::test]
async fn an_in_memory_agent_is_told_why_its_upload_cannot_run() {
    common::setup_log();
    let (server, server_addr) = server_test_node_skip_cert_verification(
        ReceiverFileTransferKernel::<StackedRatchet>(None, Arc::new(AtomicBool::new(false))),
        |b| {
            b.with_backend(test_backend());
        },
    );
    tokio::task::spawn(server);
    let agent = spawn_agent(BackendType::InMemory).await;
    tokio::time::sleep(Duration::from_millis(2000)).await;
    let (tx, mut rx, cid) = login(agent, server_addr, "john.doe").await;

    let (request_id, request) = send_file(cid, None);
    tx.send(request).unwrap();
    let message = refusal_for(&mut rx, request_id).await;
    assert!(
        message.contains("filesystem backend"),
        "refused, but not saying why: {message}"
    );
}

#[tokio::test]
async fn a_peer_sending_to_an_in_memory_agent_is_told_why_it_cannot() {
    common::setup_log();
    let (server, server_addr) =
        server_test_node_skip_cert_verification(EmptyKernel::<StackedRatchet>::default(), |b| {
            b.with_backend(test_backend());
        });
    tokio::task::spawn(server);
    let sender = spawn_agent(test_backend()).await;
    let receiver = spawn_agent(BackendType::InMemory).await;
    tokio::time::sleep(Duration::from_millis(2000)).await;
    let (mut tx_a, mut rx_a, cid_a) = login(sender, server_addr, "peer.a").await;
    let (mut tx_b, mut rx_b, cid_b) = login(receiver, server_addr, "peer.b").await;
    let settings = SessionSecuritySettingsBuilder::default().build().unwrap();
    register_p2p(
        &mut tx_a, &mut rx_a, cid_a, &mut tx_b, &mut rx_b, cid_b, settings, None,
    )
    .await
    .unwrap();
    connect_p2p(
        &mut tx_a, &mut rx_a, cid_a, &mut tx_b, &mut rx_b, cid_b, settings, None,
    )
    .await
    .unwrap();

    let (request_id, request) = send_file(cid_a, Some(cid_b));
    tx_a.send(request).unwrap();
    let message = refusal_for(&mut rx_a, request_id).await;
    assert!(
        message.contains("filesystem backend"),
        "refused, but not saying why: {message}"
    );
}

/// A pull the server cannot fulfil must be reported too. PullObject also went
/// out with a plain `send`: the client was told DownloadFileSuccess, the
/// server's refusal (a RE-VFS result carrying the error) fell into the kernel's
/// catch-all as "Unhandled node result", and the correlation registered for the
/// pull stayed queued -- where it would claim the NEXT pull's ticks. The browser
/// gave up after its own 30 s timeout, without a reason.
#[tokio::test]
async fn a_download_the_server_cannot_fulfil_is_reported() {
    common::setup_log();
    let (server, server_addr) = server_test_node_skip_cert_verification(
        ReceiverFileTransferKernel::<StackedRatchet>(None, Arc::new(AtomicBool::new(false))),
        |b| {
            b.with_backend(test_backend());
        },
    );
    tokio::task::spawn(server);
    let agent = spawn_agent(test_backend()).await;
    tokio::time::sleep(Duration::from_millis(2000)).await;
    let (tx, mut rx, cid) = login(agent, server_addr, "john.doe").await;

    let request_id = Uuid::new_v4();
    tx.send(InternalServiceRequest::DownloadFile {
        virtual_directory: PathBuf::from("/vfs/never-uploaded.txt"),
        security_level: None,
        delete_on_pull: false,
        cid,
        peer_cid: None,
        request_id,
    })
    .unwrap();
    let found = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match rx.recv().await.expect("service stream closed") {
                InternalServiceResponse::DownloadFileFailure(f) => return f,
                // "under way": the refusal must still follow it.
                InternalServiceResponse::DownloadFileSuccess(_) => continue,
                other => panic!("unexpected response while waiting for the refusal: {other:?}"),
            }
        }
    })
    .await
    .expect("DownloadFile went silent: no DownloadFileFailure within 20 s");
    assert_eq!(
        found.request_id,
        Some(request_id),
        "the refusal names another request"
    );
    assert!(!found.message.is_empty(), "refused without a reason");
}

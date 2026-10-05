//! A browser upload (`FileSource::ByteContents`) is written under the root the service was
//! given, and counted against that root's cap alone.
//!
//! The root used to be `$TMPDIR/citadel-browser-transfers` for every service in every
//! process. Payloads there are removed by a 10-minute timer, which a test run does not
//! outlive, so each run of the suite left about 35 MB behind; once the folder reached the
//! 256 MiB cap, every later upload on the machine was refused ("would exceed the ... byte
//! aggregate cap"), and `file_transfer::a_transfer_whose_link_drops_fails_on_both_sides`
//! failed on master with "B was never offered the file".

use citadel_internal_service_test_common as common;

use citadel_internal_service::kernel::CitadelWorkspaceService;
use citadel_internal_service::BrowserTransferRoot;
use citadel_internal_service_types::{FileSource, InternalServiceRequest, InternalServiceResponse};
use citadel_sdk::prefabs::server::empty::EmptyKernel;
use citadel_sdk::prelude::*;
use common::{
    get_free_port, register_and_connect_to_server, server_test_node_skip_cert_verification,
    test_backend, RegisterAndConnectItems,
};
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

/// The aggregate cap `requests/file/upload.rs` enforces.
const CAP: u64 = 256 * 1024 * 1024;

async fn agent_with(transfers: BrowserTransferRoot) -> SocketAddr {
    let addr: SocketAddr = format!("127.0.0.1:{}", get_free_port()).parse().unwrap();
    let kernel = CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(
        addr,
        citadel_internal_service::SERVER_RECONNECT,
        transfers,
    )
    .await
    .unwrap();
    let node = NodeBuilder::default()
        .with_backend(BackendType::InMemory)
        .with_node_type(NodeType::Peer)
        .with_insecure_skip_cert_verification()
        .build(kernel)
        .unwrap();
    tokio::task::spawn(node);
    addr
}

/// A private root already holding `CAP` bytes (one sparse payload, as an earlier upload
/// leaves it).
fn full_root(dir: &Path) {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir).unwrap();
    builder.create(dir.join("earlier-upload")).unwrap();
    std::fs::File::create(dir.join("earlier-upload").join("payload.bin"))
        .unwrap()
        .set_len(CAP)
        .unwrap();
}

/// Uploads a small payload from `agent`'s session and returns why it was refused.
async fn upload_refusal(agent: SocketAddr, server: SocketAddr, user: &str) -> String {
    let (tx, mut rx, cid) = register_and_connect_to_server(vec![RegisterAndConnectItems {
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
    .unwrap();
    let request_id = Uuid::new_v4();
    tx.send(InternalServiceRequest::SendFile {
        request_id,
        source: FileSource::ByteContents {
            file_name: "small.bin".to_string(),
            data: vec![7u8; 1024],
        },
        cid,
        transfer_type: TransferType::FileTransfer,
        peer_cid: None,
        chunk_size: None,
    })
    .unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match rx.recv().await.expect("service stream closed") {
                InternalServiceResponse::SendFileRequestFailure(f)
                    if f.request_id == Some(request_id) =>
                {
                    return f.message
                }
                _ => continue,
            }
        }
    })
    .await
    .expect("the upload was neither refused nor reported within 20 s")
}

#[tokio::test]
async fn an_upload_is_counted_against_its_own_services_root_only() {
    common::setup_log();
    let (server, server_addr) =
        server_test_node_skip_cert_verification(EmptyKernel::<StackedRatchet>::default(), |b| {
            b.with_backend(test_backend());
        });
    tokio::task::spawn(server);
    let full = tempfile::tempdir().unwrap();
    let empty = tempfile::tempdir().unwrap();
    let full_dir = full.path().join("transfers");
    let empty_dir = empty.path().join("transfers");
    full_root(&full_dir);
    let crowded = agent_with(BrowserTransferRoot::at(full_dir)).await;
    let roomy = agent_with(BrowserTransferRoot::at(empty_dir.clone())).await;
    tokio::time::sleep(Duration::from_millis(2000)).await;

    let refused = upload_refusal(crowded, server_addr, "crowded.user").await;
    assert!(
        refused.contains("aggregate cap"),
        "a service whose own root is full accepted the upload: {refused}"
    );

    // An in-memory agent refuses the transfer itself (no filesystem backend), but only after
    // the payload was written: under its own root, which nothing else has filled.
    let past_the_cap = upload_refusal(roomy, server_addr, "roomy.user").await;
    assert!(
        !past_the_cap.contains("aggregate cap"),
        "another service's full root refused this one's upload: {past_the_cap}"
    );
    // Written there and, since the transfer was refused, already being removed
    // (a_browser_payload_is_removed_when_its_transfer_ends.rs): the root is the evidence.
    assert!(
        empty_dir.is_dir(),
        "the payload was not written under the service's root"
    );
}

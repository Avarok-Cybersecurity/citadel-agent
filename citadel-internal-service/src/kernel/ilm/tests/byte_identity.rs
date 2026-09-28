//! The same ILM operations through the browser's store and the agent's store
//! must leave the SAME bytes under the SAME keys, or an agent-hosted ILM cannot
//! take over what a browser's ILM persisted.
//!
//! The browser side is the real thing end to end within one process: the
//! WASM client's `CitadelWorkspaceMessenger`, its bypass channel and sink, and
//! `WebSocketKvStore`, answered by a stand-in agent that serves LocalDB
//! requests the way `local_db/get_kv.rs` / `set_kv.rs` do -- over a `MemoryDb`,
//! the same stand-in the agent side writes to directly.
use super::support::MemoryDb;
use crate::kernel::ilm::kv::{AgentKvStore, LocalDbAccess};
use citadel_internal_service_connector::connector::InternalServiceConnector;
use citadel_internal_service_connector::io_interface::in_memory::InMemoryInterface;
use citadel_internal_service_connector::messenger::backend::{
    CitadelBackendExt, CitadelWorkspaceBackend,
};
use citadel_internal_service_connector::messenger::backend_ws::WebSocketKvStore;
use citadel_internal_service_connector::messenger::ilm::{Backend, MessageMetadata};
use citadel_internal_service_connector::messenger::kv_store::IlmKvStore;
use citadel_internal_service_connector::messenger::{CitadelWorkspaceMessenger, WrappedMessage};
use citadel_internal_service_types::*;
use tokio::sync::mpsc::unbounded_channel;
use uuid::Uuid;

const CID: u64 = 7;
const PEER: u64 = 9;

/// What `get_kv.rs` / `set_kv.rs` answer, over `db`, for the requests an ILM
/// backend sends. The browser always sends `peer_cid: None`; anything else
/// would address a different entry and fails the test.
async fn serve(db: &MemoryDb, request: InternalServiceRequest) -> InternalServiceResponse {
    match request {
        InternalServiceRequest::LocalDBGetKV {
            request_id,
            cid,
            peer_cid,
            key,
        } => {
            assert_eq!(peer_cid, None);
            match db.get(cid, &key).await.unwrap() {
                Some(value) => InternalServiceResponse::LocalDBGetKVSuccess(LocalDBGetKVSuccess {
                    cid,
                    peer_cid,
                    key,
                    value,
                    request_id: Some(request_id),
                }),
                None => InternalServiceResponse::LocalDBGetKVFailure(LocalDBGetKVFailure {
                    cid,
                    peer_cid,
                    message: KEY_NOT_FOUND.to_string(),
                    request_id: Some(request_id),
                }),
            }
        }
        InternalServiceRequest::LocalDBSetKV {
            request_id,
            cid,
            peer_cid,
            key,
            value,
        } => {
            assert_eq!(peer_cid, None);
            db.set(cid, &key, value).await.unwrap();
            InternalServiceResponse::LocalDBSetKVSuccess(LocalDBSetKVSuccess {
                cid,
                peer_cid,
                key,
                request_id: Some(request_id),
            })
        }
        InternalServiceRequest::Batched {
            request_id,
            commands,
        } => {
            let mut results = Vec::new();
            for command in commands {
                results.push(Box::pin(serve(db, command)).await);
            }
            InternalServiceResponse::BatchedResponse(BatchedResponseData {
                cid: 0,
                request_id: Some(request_id),
                results,
            })
        }
        other => panic!("an ILM backend sent a non-LocalDB request: {other:?}"),
    }
}

/// The browser's backend, wired through the real messenger to a stand-in
/// agent serving `db`.
async fn browser_backend(db: MemoryDb) -> CitadelWorkspaceBackend<WebSocketKvStore> {
    let (to_agent, mut agent_rx) = unbounded_channel::<InternalServiceRequest>();
    let (agent_tx, from_agent) = unbounded_channel::<InternalServiceResponse>();
    let io = InMemoryInterface::from_request_response_pair(to_agent, from_agent);
    let connector = InternalServiceConnector::from_io(io).await.unwrap();
    let (messenger, mut to_user) =
        CitadelWorkspaceMessenger::<CitadelWorkspaceBackend>::new(connector);

    let backend =
        CitadelWorkspaceBackend::with_store(CID, WebSocketKvStore::new(CID, messenger.bypasser()));

    drop(tokio::spawn(async move {
        while let Some(request) = agent_rx.recv().await {
            let _ = agent_tx.send(serve(&db, request).await);
        }
    }));
    // The messenger hands responses no registered backend claimed to the
    // user channel; this standalone backend claims its own from there.
    let claimant = backend.clone();
    drop(tokio::spawn(async move {
        let _messenger = messenger;
        while let Some(response) = to_user.recv().await {
            let leftover = claimant.inspect_received_payload(response).await.unwrap();
            assert!(
                leftover.is_none(),
                "a response nobody asked for: {leftover:?}"
            );
        }
    }));
    backend
}

fn message(source: u64, destination: u64, id: u64) -> WrappedMessage {
    WrappedMessage::construct_from_parts(
        source,
        destination,
        id,
        InternalServicePayload::Request(InternalServiceRequest::Message {
            request_id: Uuid::from_u128(id as u128),
            message: format!("body {id}").into_bytes(),
            cid: source,
            peer_cid: Some(destination),
            security_level: Default::default(),
        }),
    )
}

/// One fixed sequence covering every store operation the backend has.
async fn exercise<S: IlmKvStore>(backend: &CitadelWorkspaceBackend<S>) {
    for id in 100..103 {
        backend
            .store_outbound(message(CID, PEER, id))
            .await
            .unwrap();
    }
    backend.clear_message_outbound(PEER, 100).await.unwrap();
    backend.clear_messages_outbound(PEER, &[101]).await.unwrap();
    backend
        .store_inbound(message(PEER, CID, 500))
        .await
        .unwrap();
    backend
        .store_inbound(message(PEER, CID, 501))
        .await
        .unwrap();
    backend.clear_message_inbound(PEER, 500).await.unwrap();
    backend.store_value("tracker", b"frontier").await.unwrap();
    backend
        .store_values_batched(&[("a", vec![1, 2]), ("b", vec![3])])
        .await
        .unwrap();
    assert_eq!(
        backend.load_value("tracker").await.unwrap(),
        Some(b"frontier".to_vec())
    );
    assert_eq!(
        backend
            .load_values_batched(&["a", "b", "absent"])
            .await
            .unwrap(),
        vec![Some(vec![1, 2]), Some(vec![3]), None]
    );
}

#[tokio::test]
async fn the_browser_and_the_agent_store_the_same_bytes_under_the_same_keys() {
    let browser_db = MemoryDb::default();
    exercise(&browser_backend(browser_db.clone()).await).await;

    let agent_db = MemoryDb::default();
    exercise(&CitadelWorkspaceBackend::with_store(
        CID,
        AgentKvStore::new(CID, agent_db.clone()),
    ))
    .await;

    let browser = browser_db.snapshot();
    let keys: Vec<&str> = browser.keys().map(|(_, key)| key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "a-7",
            "b-7",
            "inbound_messages-7",
            "outbound_messages-7",
            "tracker-7"
        ],
        "the browser path did not write what the sequence should have written"
    );
    assert!(browser.keys().all(|(cid, _)| *cid == CID));
    assert_eq!(
        agent_db.snapshot(),
        browser,
        "the agent path wrote different state"
    );
}

#[tokio::test]
async fn an_agent_backend_reads_back_what_a_browser_backend_queued() {
    let db = MemoryDb::default();
    let browser = browser_backend(db.clone()).await;
    browser
        .store_outbound(message(CID, PEER, 42))
        .await
        .unwrap();
    browser.store_value("tracker", b"frontier").await.unwrap();

    let agent = CitadelWorkspaceBackend::with_store(CID, AgentKvStore::new(CID, db));
    let pending = agent.get_pending_outbound().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        (pending[0].message_id(), pending[0].destination_id()),
        (42, PEER)
    );
    assert_eq!(
        agent.load_value("tracker").await.unwrap(),
        Some(b"frontier".to_vec())
    );
}

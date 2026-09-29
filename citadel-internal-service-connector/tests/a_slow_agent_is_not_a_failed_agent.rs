//! A late answer from the local agent is still the answer.
//!
//! The messenger's backend stores ILM state in the agent's LocalDB over one
//! reliable, ordered connection, and the agent answers every LocalDB and
//! Batched request it receives. So a missing reply means exactly one thing: the
//! connection is gone. Anything else is only lateness.
//!
//! The backend used to give each reply five wall-clock seconds, then report the
//! request as failed ("Timed out writing ... may not be stored") and forget the
//! waiter, so the reply, which still came, matched nothing. A stalled agent (a
//! debug binary symbolising a backtrace, a rekey's keygen, a busy box) is not a
//! failed one: the write it was slow to confirm had been stored.
//!
//! The fake agent below answers every request correctly, a minute late. Time is
//! paused, so the minute costs nothing and the ordering against any timer the
//! backend arms is exact rather than a race.

#![cfg(not(target_arch = "wasm32"))]

use citadel_internal_service_connector::connector::InternalServiceConnector;
use citadel_internal_service_connector::io_interface::in_memory::InMemoryInterface;
use citadel_internal_service_connector::messenger::backend::CitadelWorkspaceBackend;
use citadel_internal_service_connector::messenger::{CitadelWorkspaceMessenger, IlmOptions};
use citadel_internal_service_types::{
    BatchedResponseData, InternalServiceRequest, InternalServiceResponse, LocalDBGetKVFailure,
    LocalDBGetKVSuccess, LocalDBSetKVSuccess, KEY_NOT_FOUND,
};
use citadel_io::tokio;
use citadel_io::tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use std::collections::HashMap;
use std::time::Duration;

const CID: u64 = 42;
const PEER: u64 = 43;

/// Far beyond any per-request deadline the backend has ever had.
const STALL: Duration = Duration::from_secs(60);

/// What a real agent does with the requests the backend sends, and nothing else.
fn answer(
    store: &mut HashMap<String, Vec<u8>>,
    request: InternalServiceRequest,
) -> Option<InternalServiceResponse> {
    match request {
        InternalServiceRequest::LocalDBGetKV {
            request_id,
            cid,
            peer_cid,
            key,
        } => Some(match store.get(&key) {
            Some(value) => InternalServiceResponse::LocalDBGetKVSuccess(LocalDBGetKVSuccess {
                cid,
                peer_cid,
                key,
                value: value.clone(),
                request_id: Some(request_id),
            }),
            None => InternalServiceResponse::LocalDBGetKVFailure(LocalDBGetKVFailure {
                cid,
                peer_cid,
                message: KEY_NOT_FOUND.to_string(),
                request_id: Some(request_id),
            }),
        }),
        InternalServiceRequest::LocalDBSetKV {
            request_id,
            cid,
            peer_cid,
            key,
            value,
        } => {
            store.insert(key.clone(), value);
            Some(InternalServiceResponse::LocalDBSetKVSuccess(
                LocalDBSetKVSuccess {
                    cid,
                    peer_cid,
                    key,
                    request_id: Some(request_id),
                },
            ))
        }
        InternalServiceRequest::Batched {
            request_id,
            commands,
        } => Some(InternalServiceResponse::BatchedResponse(
            BatchedResponseData {
                cid: 0,
                request_id: Some(request_id),
                results: commands
                    .into_iter()
                    .filter_map(|command| answer(store, command))
                    .collect(),
            },
        )),
        // Session polls and P2P traffic: not the backend's, and not needed here.
        _ => None,
    }
}

/// A single-threaded agent that is correct and slow: requests are served in
/// order, each after `STALL`.
async fn stalled_agent(
    mut requests: UnboundedReceiver<InternalServiceRequest>,
    responses: UnboundedSender<InternalServiceResponse>,
) {
    let mut store = HashMap::new();
    while let Some(request) = requests.recv().await {
        if let Some(response) = answer(&mut store, request) {
            tokio::time::sleep(STALL).await;
            if responses.send(response).is_err() {
                return;
            }
        }
    }
}

/// The receiver is returned so the caller holds it: a messenger whose user end
/// is gone shuts down on the first thing it forwards.
async fn messenger_over(
    requests: UnboundedSender<InternalServiceRequest>,
    responses: UnboundedReceiver<InternalServiceResponse>,
) -> (
    CitadelWorkspaceMessenger<CitadelWorkspaceBackend>,
    UnboundedReceiver<InternalServiceResponse>,
) {
    let io = InMemoryInterface::from_request_response_pair(requests, responses);
    let connector = InternalServiceConnector::from_io(io)
        .await
        .expect("in-memory connector");
    CitadelWorkspaceMessenger::new(connector, IlmOptions::LEGACY)
}

#[tokio::test(start_paused = true)]
async fn a_stalled_agent_still_completes_every_backend_request() {
    let (request_tx, request_rx) = unbounded_channel();
    let (response_tx, response_rx) = unbounded_channel();
    drop(tokio::spawn(stalled_agent(request_rx, response_tx)));
    let (messenger, mut to_user) = messenger_over(request_tx, response_rx).await;

    // ILM initialisation reads its delivery frontier and both queues through
    // the backend, and initialises the queues it finds missing: reads, a batch
    // and writes, every one answered a minute late.
    let tx = messenger
        .multiplex(CID)
        .await
        .expect("a slow LocalDB is not a failed one");

    // And a send, which is a read-modify-write of the outbound queue.
    tx.send_message_to(PEER, b"queued while the agent is slow" as &[u8])
        .await
        .expect("the queued write was stored; it was only confirmed late");

    // Every reply was the backend's own; none may leak to the application as
    // an unsolicited LocalDB answer (which is where a reply to a waiter that
    // had given up used to go).
    while let Ok(leaked) = to_user.try_recv() {
        assert!(
            !matches!(
                leaked,
                InternalServiceResponse::LocalDBGetKVSuccess(_)
                    | InternalServiceResponse::LocalDBGetKVFailure(_)
                    | InternalServiceResponse::LocalDBSetKVSuccess(_)
                    | InternalServiceResponse::BatchedResponse(_)
            ),
            "a backend reply reached the application: {leaked:?}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_lost_connection_fails_the_waiting_request_at_once() {
    // The other half of dropping the deadline: a reply that can no longer come
    // must not be waited for. The agent takes the first request and the
    // connection dies without an answer.
    let (request_tx, mut request_rx) = unbounded_channel();
    let (response_tx, response_rx) = unbounded_channel::<InternalServiceResponse>();
    drop(tokio::spawn(async move {
        let _first = request_rx.recv().await;
        drop(response_tx);
        drop(request_rx);
    }));
    let (messenger, _to_user) = messenger_over(request_tx, response_rx).await;

    let started = tokio::time::Instant::now();
    let outcome = tokio::time::timeout(Duration::from_secs(3600), messenger.multiplex(CID))
        .await
        .expect("a request whose connection is gone must not wait forever");
    assert!(
        outcome.is_err(),
        "nothing answered, so initialisation cannot have succeeded"
    );
    // Paused time: this is virtual. It separates "noticed the connection went"
    // from "a deadline expired", which is the distinction the test is about.
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the failure came from a deadline ({:?}), not from the connection loss",
        started.elapsed()
    );
}

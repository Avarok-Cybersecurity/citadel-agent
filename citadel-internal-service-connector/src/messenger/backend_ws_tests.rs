//! The browser's ILM storage traffic, pinned request by request.
//!
//! The WebSocket backend is how every browser's ILM persists its queues and
//! tracker state today, and the agent's LocalDB holds what it wrote. A change
//! here that altered a key, a value's encoding, or which request carries which
//! id would strand that state -- nothing would error, the browser would simply
//! start from an empty queue. So these tests do not ask "does it round-trip";
//! they write down the exact requests the pre-refactor backend sent for each
//! operation and require the same sequence.
//!
//! Nothing is mocked above the socket: the real backend runs, its requests go
//! out through the real `BypasserTx`, and a stand-in agent answers them from a
//! map the way `local_db/get_kv.rs` and `set_kv.rs` do.
use crate::messenger::backend::{CitadelBackendExt, CitadelWorkspaceBackend};
use crate::messenger::backend_map::State;
use crate::messenger::backend_ws::WebSocketKvStore;
use crate::messenger::{BypasserTx, InternalMessage, StreamKey, WrappedMessage, ISM_STREAM_ID};
use citadel_internal_service_types::{
    BatchedResponseData, InternalServicePayload, InternalServiceRequest, InternalServiceResponse,
    LocalDBGetKVFailure, LocalDBGetKVSuccess, LocalDBSetKVSuccess, KEY_NOT_FOUND,
};
use citadel_io::tokio;
use intersession_layer_messaging::{Backend, Payload};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

const CID: u64 = 7;
const PEER: u64 = 9;

/// A LocalDB request as the agent receives it, minus the random request ids.
#[derive(Debug, Clone, PartialEq)]
enum Seen {
    Get {
        cid: u64,
        peer_cid: Option<u64>,
        key: String,
    },
    Set {
        cid: u64,
        peer_cid: Option<u64>,
        key: String,
        value: Vec<u8>,
    },
    Batch(Vec<Seen>),
}

fn get(key: &str) -> Seen {
    Seen::Get {
        cid: CID,
        peer_cid: None,
        key: key.to_string(),
    }
}

fn set(key: &str, value: Vec<u8>) -> Seen {
    Seen::Set {
        cid: CID,
        peer_cid: None,
        key: key.to_string(),
        value,
    }
}

/// The only line that differs from the pre-refactor version of this test,
/// which constructed the backend's old private fields directly and passed.
fn make_backend(outbound: BypasserTx) -> CitadelWorkspaceBackend {
    CitadelWorkspaceBackend::with_store(CID, WebSocketKvStore::new(CID, outbound))
}

#[derive(Default)]
struct Agent {
    kv: HashMap<String, Vec<u8>>,
    seen: Vec<Seen>,
    set_request_ids: Vec<Uuid>,
}

impl Agent {
    fn answer(&mut self, request: InternalServiceRequest) -> (Seen, InternalServiceResponse) {
        match request {
            InternalServiceRequest::LocalDBGetKV {
                request_id,
                cid,
                peer_cid,
                key,
            } => {
                let seen = Seen::Get {
                    cid,
                    peer_cid,
                    key: key.clone(),
                };
                let response = match self.kv.get(&key) {
                    Some(value) => {
                        InternalServiceResponse::LocalDBGetKVSuccess(LocalDBGetKVSuccess {
                            cid,
                            peer_cid,
                            key,
                            value: value.clone(),
                            request_id: Some(request_id),
                        })
                    }
                    None => InternalServiceResponse::LocalDBGetKVFailure(LocalDBGetKVFailure {
                        cid,
                        peer_cid,
                        message: KEY_NOT_FOUND.to_string(),
                        request_id: Some(request_id),
                    }),
                };
                (seen, response)
            }
            InternalServiceRequest::LocalDBSetKV {
                request_id,
                cid,
                peer_cid,
                key,
                value,
            } => {
                self.set_request_ids.push(request_id);
                let seen = Seen::Set {
                    cid,
                    peer_cid,
                    key: key.clone(),
                    value: value.clone(),
                };
                self.kv.insert(key.clone(), value);
                let response = InternalServiceResponse::LocalDBSetKVSuccess(LocalDBSetKVSuccess {
                    cid,
                    peer_cid,
                    key,
                    request_id: Some(request_id),
                });
                (seen, response)
            }
            InternalServiceRequest::Batched {
                request_id,
                commands,
            } => {
                let (seen, results): (Vec<_>, Vec<_>) =
                    commands.into_iter().map(|c| self.answer(c)).unzip();
                let response = InternalServiceResponse::BatchedResponse(BatchedResponseData {
                    cid: 0,
                    request_id: Some(request_id),
                    results,
                });
                (Seen::Batch(seen), response)
            }
            other => panic!("the ILM backend sent a non-LocalDB request: {other:?}"),
        }
    }
}

/// Starts the stand-in agent and returns the backend wired to it.
fn start() -> (CitadelWorkspaceBackend, Arc<Mutex<Agent>>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(StreamKey, InternalMessage)>();
    let backend = make_backend(BypasserTx {
        tx,
        stream_key: StreamKey {
            cid: CID,
            stream_id: ISM_STREAM_ID,
        },
    });
    let agent = Arc::new(Mutex::new(Agent::default()));
    let (task_backend, task_agent) = (backend.clone(), agent.clone());
    drop(tokio::spawn(async move {
        while let Some((key, payload)) = rx.recv().await {
            assert_eq!(key, StreamKey::bypass_ism(), "storage must bypass ILM");
            let Payload::Message(WrappedMessage {
                contents: InternalServicePayload::Request(request),
                ..
            }) = payload
            else {
                panic!("the backend sent something that is not a request");
            };
            let (seen, response) = task_agent.lock().unwrap().answer(request);
            task_agent.lock().unwrap().seen.push(seen);
            let leftover = task_backend
                .inspect_received_payload(response)
                .await
                .unwrap();
            assert!(
                leftover.is_none(),
                "the backend did not claim its own response"
            );
        }
    }));
    (backend, agent)
}

fn message(message_id: u64, request_id: Uuid) -> WrappedMessage {
    WrappedMessage {
        source_id: CID,
        destination_id: PEER,
        message_id,
        contents: InternalServicePayload::Request(InternalServiceRequest::Message {
            request_id,
            message: b"hello".to_vec(),
            cid: CID,
            peer_cid: Some(PEER),
            security_level: Default::default(),
        }),
    }
}

fn state_bytes(state: &State) -> Vec<u8> {
    bincode2::serialize(state).unwrap()
}

fn take_seen(agent: &Mutex<Agent>) -> Vec<Seen> {
    std::mem::take(&mut agent.lock().unwrap().seen)
}

#[tokio::test]
async fn queue_mutations_send_the_same_requests_as_before() {
    let (backend, agent) = start();
    let request_id = Uuid::new_v4();
    let queued = message(100, request_id);

    backend.store_outbound(queued.clone()).await.unwrap();

    let mut with_message = State::new();
    with_message.entry(PEER).or_default().insert(100, queued);
    assert_eq!(
        take_seen(&agent),
        vec![
            get("outbound_messages-7"),
            // Absent map: initialised empty, then written with the message.
            set("outbound_messages-7", state_bytes(&State::new())),
            set("outbound_messages-7", state_bytes(&with_message)),
        ]
    );
    // The queue write carries the MESSAGE's request id, as it always has.
    assert_eq!(
        agent.lock().unwrap().set_request_ids.last(),
        Some(&request_id)
    );

    backend.clear_message_outbound(PEER, 100).await.unwrap();
    let mut cleared = State::new();
    cleared.entry(PEER).or_default();
    assert_eq!(
        take_seen(&agent),
        vec![
            get("outbound_messages-7"),
            set("outbound_messages-7", state_bytes(&cleared))
        ]
    );

    assert!(backend.get_pending_outbound().await.unwrap().is_empty());
    assert_eq!(take_seen(&agent), vec![get("outbound_messages-7")]);
}

#[tokio::test]
async fn tracker_values_send_the_same_requests_as_before() {
    let (backend, agent) = start();

    assert_eq!(backend.load_value("tracker").await.unwrap(), None);
    backend.store_value("tracker", b"abc").await.unwrap();
    assert_eq!(
        backend.load_value("tracker").await.unwrap(),
        Some(b"abc".to_vec())
    );
    assert_eq!(
        take_seen(&agent),
        vec![
            get("tracker-7"),
            set("tracker-7", b"abc".to_vec()),
            get("tracker-7")
        ]
    );

    backend
        .store_values_batched(&[("a", vec![1]), ("b", vec![2])])
        .await
        .unwrap();
    let loaded = backend
        .load_values_batched(&["a", "b", "missing"])
        .await
        .unwrap();
    assert_eq!(loaded, vec![Some(vec![1]), Some(vec![2]), None]);
    assert_eq!(
        take_seen(&agent),
        vec![
            Seen::Batch(vec![set("a-7", vec![1]), set("b-7", vec![2])]),
            Seen::Batch(vec![get("a-7"), get("b-7"), get("missing-7")]),
        ]
    );

    // Empty batches never reach the agent.
    backend.store_values_batched(&[]).await.unwrap();
    assert!(backend.load_values_batched(&[]).await.unwrap().is_empty());
    assert!(take_seen(&agent).is_empty());
}

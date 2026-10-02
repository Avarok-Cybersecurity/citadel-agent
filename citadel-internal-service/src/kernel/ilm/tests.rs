//! Two agent-hosted ILMs wired to each other through a double of the agent.
//!
//! No SDK and no sockets: each account's store is a map, and a frame one
//! account's transport sends is fed to the other's registry exactly as the
//! agent's P2P read stream would feed it. What is real is everything this
//! module assembles: ILM, the shared backend, the shared wire encoding.

use super::*;
use citadel_internal_service_connector::messenger::backend::CitadelWorkspaceBackend;
use citadel_internal_service_types::{
    BatchedResponseData, LocalDBGetKVFailure, LocalDBGetKVSuccess, LocalDBSetKVSuccess,
    SecurityLevel, KEY_NOT_FOUND,
};
use intersession_layer_messaging::Backend;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

const ALICE: u64 = 1001;
const BOB: u64 = 2002;
const PATIENCE: Duration = Duration::from_secs(20);

struct FakeAgent {
    cid: u64,
    peer: u64,
    store: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    peer_registry: Mutex<Weak<IlmRegistry>>,
    accepts: AtomicBool,
    delivered: UnboundedSender<MessageNotification>,
}

impl FakeAgent {
    fn answer(&self, request: InternalServiceRequest) -> InternalServiceResponse {
        match request {
            InternalServiceRequest::LocalDBGetKV {
                request_id,
                cid,
                peer_cid,
                key,
            } => match self.store.lock().get(&key).cloned() {
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
            },
            InternalServiceRequest::LocalDBSetKV {
                request_id,
                cid,
                peer_cid,
                key,
                value,
            } => {
                self.store.lock().insert(key.clone(), value);
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
            } => InternalServiceResponse::BatchedResponse(BatchedResponseData {
                cid: 0,
                request_id: Some(request_id),
                results: commands.into_iter().map(|c| self.answer(c)).collect(),
            }),
            other => panic!("the ILM made a request the agent does not answer for it: {other:?}"),
        }
    }
}

impl HostIo for FakeAgent {
    fn local_db(
        &self,
        request: InternalServiceRequest,
    ) -> BoxFuture<'static, InternalServiceResponse> {
        let answer = self.answer(request);
        Box::pin(async move { answer })
    }

    fn send_frame(
        &self,
        request: InternalServiceRequest,
    ) -> BoxFuture<'static, Result<(), String>> {
        let InternalServiceRequest::Message {
            message,
            cid,
            peer_cid,
            ..
        } = request
        else {
            panic!("an ILM frame is always a Message")
        };
        assert_eq!(cid, self.cid, "a frame left under the wrong account");
        assert_eq!(peer_cid, Some(self.peer), "a frame left for the wrong peer");
        // What the peer's agent read stream produces from these bytes.
        let arrived = MessageNotification {
            message,
            cid: self.peer,
            peer_cid: self.cid,
            request_id: None,
        };
        let peer = self.peer_registry.lock().upgrade();
        Box::pin(async move {
            if let Some(registry) = peer {
                // A frame for an unhosted account is the raw path's; none here.
                assert!(
                    registry.feed(arrived).is_ok(),
                    "the peer did not take an ILM frame"
                );
            }
            Ok(())
        })
    }

    fn connected_peers(&self, _cid: u64) -> Vec<u64> {
        vec![self.peer]
    }

    fn deliver(&self, cid: u64, notification: MessageNotification) -> BoxFuture<'static, bool> {
        assert_eq!(cid, self.cid);
        let taken =
            self.accepts.load(Ordering::SeqCst) && self.delivered.send(notification).is_ok();
        Box::pin(async move { taken })
    }
}

struct Account {
    io: Arc<FakeAgent>,
    registry: Arc<IlmRegistry>,
    delivered: UnboundedReceiver<MessageNotification>,
}

fn account(cid: u64, peer: u64) -> Account {
    let (delivered_tx, delivered) = unbounded_channel();
    let io = Arc::new(FakeAgent {
        cid,
        peer,
        store: Default::default(),
        peer_registry: Mutex::new(Weak::new()),
        accepts: AtomicBool::new(true),
        delivered: delivered_tx,
    });
    Account {
        io,
        registry: Arc::new(IlmRegistry::default()),
        delivered,
    }
}

fn pair() -> (Account, Account) {
    let alice = account(ALICE, BOB);
    let bob = account(BOB, ALICE);
    *alice.io.peer_registry.lock() = Arc::downgrade(&bob.registry);
    *bob.io.peer_registry.lock() = Arc::downgrade(&alice.registry);
    (alice, bob)
}

async fn host(account: &Account) -> Arc<registry::IlmHost> {
    account
        .registry
        .ensure(account.io.cid, account.io.clone())
        .await
        .expect("the ILM starts")
        .expect("no other start in progress")
}

async fn next(rx: &mut UnboundedReceiver<MessageNotification>) -> MessageNotification {
    tokio::time::timeout(PATIENCE, rx.recv())
        .await
        .expect("nothing was delivered in time")
        .expect("delivery channel open")
}

async fn send(host: &registry::IlmHost, to: u64, body: &[u8]) {
    host.send(
        to,
        Uuid::new_v4(),
        body.to_vec(),
        SecurityLevel::Standard,
        None,
    )
    .await
    .expect("ILM accepts the message");
}

#[path = "tests_cases.rs"]
mod cases;

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

    fn deliver(&self, cid: u64, notification: MessageNotification) -> bool {
        assert_eq!(cid, self.cid);
        self.accepts.load(Ordering::SeqCst) && self.delivered.send(notification).is_ok()
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

#[tokio::test]
async fn a_message_crosses_between_two_agent_hosted_ilms_once_and_in_order() {
    let (mut alice, mut bob) = pair();
    let (a, b) = (host(&alice).await, host(&bob).await);

    for body in [b"one".as_slice(), b"two", b"three"] {
        send(&a, BOB, body).await;
    }
    for expected in [b"one".as_slice(), b"two", b"three"] {
        let got = next(&mut bob.delivered).await;
        assert_eq!(got.message, expected, "out of order or altered");
        assert_eq!((got.cid, got.peer_cid), (BOB, ALICE));
    }
    send(&b, ALICE, b"reply").await;
    assert_eq!(next(&mut alice.delivered).await.message, b"reply");
    assert!(
        tokio::time::timeout(Duration::from_millis(1500), bob.delivered.recv())
            .await
            .is_err(),
        "a message was delivered twice"
    );
}

/// No window attached: delivery is refused, the message is kept (and not
/// acknowledged), and it arrives the moment someone can take it.
#[tokio::test]
async fn a_message_nobody_can_take_yet_is_kept_not_lost() {
    let (alice, mut bob) = pair();
    let a = host(&alice).await;
    host(&bob).await;
    bob.io.accepts.store(false, Ordering::SeqCst);

    send(&a, BOB, b"while away").await;
    assert!(
        tokio::time::timeout(Duration::from_millis(1500), bob.delivered.recv())
            .await
            .is_err(),
        "delivered although nobody could take it"
    );
    bob.io.accepts.store(true, Ordering::SeqCst);
    assert_eq!(next(&mut bob.delivered).await.message, b"while away");
}

/// What an earlier ILM for the account left queued under the account's keys is
/// picked up and sent by the next one. A browser's ILM writes those same keys
/// through the same backend (the connector's `through_any_channel` tests), so
/// this is the hand-over from a browser to the agent: nothing is lost.
#[tokio::test]
async fn a_new_host_sends_what_an_earlier_one_left_queued() {
    let (alice, mut bob) = pair();
    host(&bob).await;
    // Bob is unreachable while the first ILM runs: its frames go nowhere.
    let reachable_bob = std::mem::take(&mut *alice.io.peer_registry.lock());
    {
        let first = host(&alice).await;
        send(&first, BOB, b"queued, never sent").await;
    }
    alice.registry.stop(ALICE);
    let pending =
        CitadelWorkspaceBackend::with_channel(ALICE, channel::AgentChannel::new(alice.io.clone()))
            .get_pending_outbound()
            .await
            .expect("the account's store is readable");
    assert_eq!(pending.len(), 1, "the message was not left queued");
    assert!(
        bob.delivered.try_recv().is_err(),
        "it reached Bob before the hand-over"
    );

    *alice.io.peer_registry.lock() = reachable_bob;
    host(&alice).await;
    assert_eq!(
        next(&mut bob.delivered).await.message,
        b"queued, never sent"
    );
}

#[tokio::test]
async fn one_ilm_per_account() {
    let (alice, _bob) = pair();
    let first = host(&alice).await;
    let again = host(&alice).await;
    assert!(
        Arc::ptr_eq(&first, &again),
        "a second ILM was started for one account"
    );
    alice.registry.stop(ALICE);
    assert!(!alice.registry.is_hosted(ALICE));
    let fresh = host(&alice).await;
    assert!(!Arc::ptr_eq(&first, &fresh));
}

/// Raw traffic (Yjs, the plain messaging service) and unhosted accounts are not
/// the ILM's: the notification comes back untouched for the raw path.
#[tokio::test]
async fn what_is_not_an_ilm_frame_is_handed_back() {
    let (alice, _bob) = pair();
    let raw = MessageNotification {
        message: b"yjs update".to_vec(),
        cid: ALICE,
        peer_cid: BOB,
        request_id: None,
    };
    let handed_back = |result: Result<(), MessageNotification>| match result {
        Err(n) => n.message == raw.message && n.cid == raw.cid,
        Ok(()) => false,
    };
    assert!(handed_back(alice.registry.feed(raw.clone())), "unhosted");
    host(&alice).await;
    assert!(handed_back(alice.registry.feed(raw.clone())), "not a frame");
}

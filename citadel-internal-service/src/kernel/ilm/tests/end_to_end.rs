//! Two agent-hosted ILMs, real ILM on both ends, exchanging a message and its
//! acknowledgement over the stand-in peer channels in `support.rs`.
use super::support::{Loopback, MemoryDb};
use crate::kernel::ilm::host::AgentIlmHost;
use crate::kernel::ilm::kv::AgentKvStore;
use citadel_internal_service_connector::messenger::backend::CitadelWorkspaceBackend;
use citadel_internal_service_connector::messenger::ilm::{Backend, Payload};
use citadel_internal_service_connector::messenger::wire::{decode_inbound, InboundFrame};
use citadel_internal_service_types::{
    InternalServicePayload, InternalServiceRequest, InternalServiceResponse, MessageNotification,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use uuid::Uuid;

const ALICE: u64 = 1001;
const BOB: u64 = 2002;
const PATIENCE: Duration = Duration::from_secs(10);

type Host = Arc<AgentIlmHost<MemoryDb, Loopback>>;

async fn host(
    cid: u64,
    db: MemoryDb,
    wire: &Loopback,
) -> (Host, UnboundedReceiver<InternalServiceResponse>) {
    let (delivered, rx) = unbounded_channel();
    let host = Arc::new(
        AgentIlmHost::start(cid, db, wire.clone(), delivered)
            .await
            .unwrap(),
    );
    wire.attach(host.clone());
    (host, rx)
}

fn outgoing(from: u64, to: u64, body: &[u8]) -> InternalServicePayload {
    InternalServicePayload::Request(InternalServiceRequest::Message {
        request_id: Uuid::new_v4(),
        message: body.to_vec(),
        cid: from,
        peer_cid: Some(to),
        security_level: Default::default(),
    })
}

async fn next_delivery(rx: &mut UnboundedReceiver<InternalServiceResponse>) -> MessageNotification {
    match tokio::time::timeout(PATIENCE, rx.recv()).await {
        Ok(Some(InternalServiceResponse::MessageNotification(n))) => n,
        other => panic!("expected a delivered message, got {other:?}"),
    }
}

/// True once `cid`'s durable outbound queue holds nothing: the sender only
/// clears a message when the receiver's ACK arrives.
async fn outbound_drains(cid: u64, db: MemoryDb) -> bool {
    let view = CitadelWorkspaceBackend::with_store(cid, AgentKvStore::new(cid, db));
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while tokio::time::Instant::now() < deadline {
        if view.get_pending_outbound().await.unwrap().is_empty() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

fn acks_sent(wire: &Loopback, from: u64, to: u64) -> usize {
    wire.sent
        .lock()
        .unwrap()
        .iter()
        .filter(|sent| (sent.from, sent.to) == (from, to))
        .filter(|sent| {
            let arrived = MessageNotification {
                message: sent.bytes.clone(),
                cid: sent.to,
                peer_cid: sent.from,
                request_id: None,
            };
            matches!(
                decode_inbound(arrived),
                Ok(InboundFrame::Signal(Payload::Ack { .. }))
            )
        })
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn two_agent_hosted_ilms_deliver_and_acknowledge() {
    let wire = Loopback::default();
    let (alice_db, bob_db) = (MemoryDb::default(), MemoryDb::default());
    let (alice, mut alice_inbox) = host(ALICE, alice_db.clone(), &wire).await;
    let (bob, mut bob_inbox) = host(BOB, bob_db.clone(), &wire).await;

    alice
        .ilm()
        .send_to(BOB, outgoing(ALICE, BOB, b"hello bob"))
        .await
        .unwrap();

    let received = next_delivery(&mut bob_inbox).await;
    assert_eq!(received.message, b"hello bob");
    assert_eq!((received.cid, received.peer_cid), (BOB, ALICE));

    assert!(
        outbound_drains(ALICE, alice_db).await,
        "Alice's queue never drained: no ACK"
    );
    assert!(acks_sent(&wire, BOB, ALICE) > 0, "Bob sent no ACK frame");

    // And back, so both directions of both transports are exercised.
    bob.ilm()
        .send_to(ALICE, outgoing(BOB, ALICE, b"hi alice"))
        .await
        .unwrap();
    let reply = next_delivery(&mut alice_inbox).await;
    assert_eq!(reply.message, b"hi alice");
    assert_eq!((reply.cid, reply.peer_cid), (ALICE, BOB));
    assert!(
        outbound_drains(BOB, bob_db).await,
        "Bob's queue never drained: no ACK"
    );
}

//! What opting in changes on the wire, through the production decisions
//! (`AgentIlmService::send_reliable` / `route_inbound`): an opted-in account's
//! frames go to its agent's ILM and come out once, unwrapped; everything else
//! keeps the raw path untouched; stopping hands the account back to it.
//! `MemoryDb` / `Loopback` stand in for the node store and peer sinks; see
//! support.rs for why each is needed.
use super::opt_in::{frame, Service, ALICE, BOB};
use super::support::{Loopback, MemoryDb};
use crate::kernel::ilm::service::{Inbound, MultiSubscriber, SendReliableError};
use citadel_internal_service_types::{InternalServiceResponse, SecurityLevel};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use uuid::Uuid;

async fn hosted(
    cid: u64,
    wire: &Loopback,
) -> (Arc<Service>, UnboundedReceiver<InternalServiceResponse>) {
    let service = Arc::new(Service::new(MultiSubscriber::On));
    let (delivered, inbox) = unbounded_channel();
    service
        .enable(cid, MemoryDb::default(), wire.clone(), delivered)
        .await
        .unwrap();
    wire.attach_service(cid, service.clone());
    (service, inbox)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reliable_send_arrives_once_unwrapped_through_the_peers_inbound_route() {
    let wire = Loopback::default();
    let (alice, _alice_inbox) = hosted(ALICE, &wire).await;
    let (_bob, mut bob_inbox) = hosted(BOB, &wire).await;

    alice
        .send_reliable(
            Uuid::new_v4(),
            ALICE,
            BOB,
            b"hello bob".to_vec(),
            SecurityLevel::Standard,
        )
        .await
        .unwrap();

    // attach_service panics if Bob's inbound decision sent a frame down the raw path.
    let delivered = tokio::time::timeout(Duration::from_secs(10), bob_inbox.recv()).await;
    let Ok(Some(InternalServiceResponse::MessageNotification(message))) = delivered else {
        panic!("Bob's agent delivered nothing: {delivered:?}");
    };
    assert_eq!((message.cid, message.peer_cid), (BOB, ALICE));
    assert_eq!(&message.message[..], b"hello bob");

    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(bob_inbox.try_recv().is_err(), "delivered more than once");
}

#[tokio::test]
async fn an_account_that_has_not_opted_in_keeps_the_raw_path_untouched() {
    let wire = Loopback::default();
    let (service, _inbox) = hosted(ALICE, &wire).await;
    let arriving = frame(ALICE, BOB, b"for bob");

    let Inbound::Raw(forwarded) = service.route_inbound(arriving.clone()) else {
        panic!("a frame for an account that never opted in was taken");
    };
    assert_eq!(
        (
            forwarded.cid,
            forwarded.peer_cid,
            &forwarded.message[..],
            forwarded.request_id
        ),
        (
            arriving.cid,
            arriving.peer_cid,
            &arriving.message[..],
            arriving.request_id
        )
    );
}

#[tokio::test]
async fn what_is_not_an_ilm_frame_takes_the_raw_path_even_when_opted_in() {
    let wire = Loopback::default();
    let (service, _inbox) = hosted(BOB, &wire).await;
    let mut raw = frame(ALICE, BOB, b"");
    raw.message = b"a Yjs update, not ILM's".to_vec();

    assert!(matches!(service.route_inbound(raw), Inbound::Raw(_)));
}

#[tokio::test]
async fn stopping_hands_the_account_back_to_the_raw_path() {
    let wire = Loopback::default();
    let (service, _inbox) = hosted(BOB, &wire).await;
    assert!(service.stop(BOB));

    assert_eq!(service.offer().unwrap().hosted, Vec::<u64>::new());
    assert!(matches!(
        service.route_inbound(frame(ALICE, BOB, b"x")),
        Inbound::Raw(_)
    ));
    let refused = service
        .send_reliable(
            Uuid::new_v4(),
            BOB,
            ALICE,
            b"x".to_vec(),
            SecurityLevel::Standard,
        )
        .await;
    assert!(
        matches!(refused, Err(SendReliableError::NotOptedIn)),
        "{refused:?}"
    );
}

//! The opt-in decisions, over the real ILM: whether it is offered, what a
//! second opt-in does, and that a reliable send never falls back to raw.
//! `MemoryDb` / `Loopback` stand in for the node store and peer sinks; see
//! support.rs for why each is needed.
use super::support::{Loopback, MemoryDb};
use crate::kernel::ilm::service::{
    AgentIlmService, EnableError, Enabled, Inbound, MultiSubscriber, SendReliableError,
};
use citadel_internal_service_connector::messenger::WireWrapper;
use citadel_internal_service_types::{AgentIlmOffer, MessageNotification, SecurityLevel};
use tokio::sync::mpsc::unbounded_channel;
use uuid::Uuid;

pub(super) const ALICE: u64 = 1001;
pub(super) const BOB: u64 = 2002;

pub(super) type Service = AgentIlmService<MemoryDb, Loopback>;

/// An ILM message frame from `from` to `to`, as the peer's ILM puts it on the wire.
pub(super) fn frame(from: u64, to: u64, body: &[u8]) -> MessageNotification {
    let wire = WireWrapper::Message {
        contents: body.to_vec(),
        source: from,
        destination: to,
        message_id: 0,
    };
    MessageNotification {
        message: bincode2::serialize(&wire).unwrap(),
        cid: to,
        peer_cid: from,
        request_id: None,
    }
}

#[tokio::test]
async fn with_the_setting_off_nothing_is_offered_and_opt_in_is_refused() {
    let service = Service::new(MultiSubscriber::Off);
    assert_eq!(
        service.offer(),
        None,
        "an Off agent advertised the capability"
    );

    let (delivered, _inbox) = unbounded_channel();
    let refused = service
        .enable(BOB, MemoryDb::default(), Loopback::default(), delivered)
        .await;
    assert!(
        matches!(refused, Err(EnableError::NotOffered)),
        "{refused:?}"
    );
    assert!(!service.is_hosted(BOB));
    // And so every inbound message keeps the raw path.
    assert!(matches!(
        service.route_inbound(frame(ALICE, BOB, b"x")),
        Inbound::Raw(_)
    ));
}

#[tokio::test]
async fn with_the_setting_on_the_offer_lists_the_hosted_accounts() {
    let service = Service::new(MultiSubscriber::On);
    assert_eq!(service.offer(), Some(AgentIlmOffer { hosted: vec![] }));

    let (delivered, _inbox) = unbounded_channel();
    service
        .enable(BOB, MemoryDb::default(), Loopback::default(), delivered)
        .await
        .unwrap();
    assert_eq!(service.offer(), Some(AgentIlmOffer { hosted: vec![BOB] }));
}

/// Pinned as IDEMPOTENT: a reloaded page cannot know the agent's ILM survived
/// it except by asking, so asking again must succeed -- and must not start a
/// second ILM over the same keys.
#[tokio::test]
async fn opting_in_twice_is_idempotent_and_starts_one_ilm() {
    let service = Service::new(MultiSubscriber::On);
    let (delivered, _inbox) = unbounded_channel();
    let first = service
        .enable(
            BOB,
            MemoryDb::default(),
            Loopback::default(),
            delivered.clone(),
        )
        .await;
    assert_eq!(first.unwrap(), Enabled::Started);

    let second = service
        .enable(BOB, MemoryDb::default(), Loopback::default(), delivered)
        .await;
    assert_eq!(second.unwrap(), Enabled::AlreadyHosted);
    assert_eq!(service.offer().unwrap().hosted, vec![BOB]);
}

#[tokio::test]
async fn a_reliable_send_before_opting_in_is_refused_and_sends_nothing() {
    let service = Service::new(MultiSubscriber::On);
    let wire = Loopback::default();
    let (delivered, _inbox) = unbounded_channel();
    // Another account is hosted, over the same wire: only ALICE is not.
    service
        .enable(BOB, MemoryDb::default(), wire.clone(), delivered)
        .await
        .unwrap();

    let refused = service
        .send_reliable(
            Uuid::new_v4(),
            ALICE,
            BOB,
            b"hi".to_vec(),
            SecurityLevel::Standard,
        )
        .await;
    assert!(
        matches!(refused, Err(SendReliableError::NotOptedIn)),
        "{refused:?}"
    );
    assert!(
        wire.sent.lock().unwrap().is_empty(),
        "a refused reliable send still put a frame on the wire"
    );
}

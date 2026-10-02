//! What the conversation store asks the agent to raise a notice for.

use super::engine::Engine;
use super::engine_fake::*;
use crate::kernel::notices::decide::NoticeSource;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/p2p_commands/{name}.cbor",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[tokio::test]
async fn an_arriving_chat_message_asks_for_a_notice_once() {
    let agent = FakeAgent::with_window();
    let engine = Engine::default();
    assert!(
        engine
            .delivered(&agent, from_peer(chat("m1", "hi there")))
            .await
    );
    assert!(
        engine
            .delivered(&agent, from_peer(chat("m1", "hi there")))
            .await
    );
    assert_eq!(
        *agent.notices.lock(),
        vec![NoticeSource::Message {
            peer: PEER,
            peer_username: Some("bob".into()),
            text: "hi there".into()
        }]
    );
}

#[tokio::test]
async fn a_ring_asks_for_a_notice_and_a_hang_up_does_not() {
    let agent = FakeAgent::with_window();
    let engine = Engine::default();
    engine
        .delivered(&agent, from_peer(fixture("call-signal")))
        .await;
    assert!(agent.notices.lock().is_empty());
    engine
        .delivered(&agent, from_peer(fixture("call-invite")))
        .await;
    assert_eq!(
        *agent.notices.lock(),
        vec![NoticeSource::IncomingCall {
            peer: PEER,
            peer_username: Some("bob".into())
        }]
    );
}

#[tokio::test]
async fn typing_asks_for_nothing() {
    let agent = FakeAgent::with_window();
    Engine::default()
        .delivered(&agent, from_peer(fixture("typing")))
        .await;
    assert!(agent.notices.lock().is_empty());
}

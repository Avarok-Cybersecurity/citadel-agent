//! The conversation store as a whole, against the double in engine_fake.rs.

use super::command::{self, AckKind, Inbound};
use super::engine::{save_preferences, store, Engine};
use super::engine_fake::*;
use citadel_internal_service_types::{AccountPreferences, ConversationEventKind, MessageStatus};
use std::sync::atomic::{AtomicBool, Ordering};

#[tokio::test]
async fn an_arriving_message_is_stored_receipted_and_announced() {
    let agent = FakeAgent::with_window();
    let engine = Engine::default();
    assert!(
        engine
            .delivered(&agent, from_peer(chat("m1", "hello there")))
            .await
    );

    assert_eq!(
        status_of(&agent, "m1").await,
        Some(MessageStatus::Delivered)
    );
    assert_eq!(
        agent.events(),
        vec![(
            ConversationEventKind::Appended,
            Some("m1".into()),
            "hello there".into()
        )]
    );
    assert_eq!(
        agent.sent_commands(),
        vec![Inbound::Ack {
            kind: AckKind::Delivered,
            message_id: "m1".into()
        }]
    );
    let meta = store(&agent, ME)
        .load_metadata(PEER)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (meta.unread_count, meta.peer_username.as_deref()),
        (1.0, Some("bob"))
    );
}

/// No window open: the message is stored and receipted all the same.
#[tokio::test]
async fn a_message_is_stored_with_no_window_open() {
    let agent = FakeAgent {
        known: AtomicBool::new(true),
        ..FakeAgent::default()
    };
    assert!(
        Engine::default()
            .delivered(&agent, from_peer(chat("m1", "while away")))
            .await
    );
    assert_eq!(
        status_of(&agent, "m1").await,
        Some(MessageStatus::Delivered)
    );
    assert_eq!(agent.sent_commands().len(), 1);
}

#[tokio::test]
async fn a_redelivery_is_receipted_again_but_stored_and_announced_once() {
    let agent = FakeAgent::with_window();
    let engine = Engine::default();
    engine
        .delivered(&agent, from_peer(chat("m1", "once")))
        .await;
    engine
        .delivered(&agent, from_peer(chat("m1", "once")))
        .await;
    assert_eq!(agent.events().len(), 1);
    assert_eq!(agent.sent_commands().len(), 2);
}

#[tokio::test]
async fn a_strangers_message_is_hidden_when_the_account_refuses_strangers() {
    let agent = FakeAgent::with_window();
    agent.known.store(false, Ordering::SeqCst);
    let prefs = AccountPreferences {
        accept_requests_from_strangers: false,
        ..AccountPreferences::UI_DEFAULTS
    };
    save_preferences(&agent, ME, &prefs).await.unwrap();
    assert!(
        Engine::default()
            .delivered(&agent, from_peer(chat("s1", "hi")))
            .await
    );
    assert_eq!(status_of(&agent, "s1").await, None);
    assert!(
        agent.sent.lock().is_empty(),
        "a hidden message was receipted"
    );
}

/// Storage failing is "not yet": ILM must deliver it again.
#[tokio::test]
async fn a_message_that_cannot_be_stored_is_not_acknowledged() {
    let agent = FakeAgent::with_window();
    agent.kv.1.store(true, Ordering::SeqCst);
    assert!(
        !Engine::default()
            .delivered(&agent, from_peer(chat("m1", "x")))
            .await
    );
    assert!(agent.sent.lock().is_empty());
}

#[tokio::test]
async fn sending_numbers_stores_and_sends_the_message() {
    let agent = FakeAgent::with_window();
    let engine = Engine::default();
    let first = engine
        .send(&agent, ME, PEER, outgoing("one"))
        .await
        .unwrap()
        .unwrap();
    let second = engine
        .send(&agent, ME, PEER, outgoing("two"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!((first.index, second.index), (1.0, 2.0));
    assert_eq!(
        (first.status, second.status),
        (MessageStatus::Sent, MessageStatus::Sent)
    );
    let Inbound::Message {
        envelope, contents, ..
    } = &agent.sent_commands()[1]
    else {
        panic!()
    };
    assert_eq!(
        (
            envelope.message_id.as_str(),
            envelope.index,
            contents.as_str()
        ),
        (second.id.as_str(), 2.0, "two")
    );
    let kinds: Vec<_> = agent.events().into_iter().map(|e| e.0).collect();
    use ConversationEventKind::*;
    assert_eq!(kinds, [Appended, Updated, Appended, Updated]);
}

#[tokio::test]
async fn a_send_that_fails_is_stored_failed_and_can_be_sent_again() {
    let agent = FakeAgent::with_window();
    let engine = Engine::default();
    agent.link_down.store(true, Ordering::SeqCst);
    assert!(engine
        .send(&agent, ME, PEER, outgoing("hello"))
        .await
        .is_err());
    let id = "id-0";
    assert_eq!(status_of(&agent, id).await, Some(MessageStatus::Failed));

    agent.link_down.store(false, Ordering::SeqCst);
    let resent = engine.resend(&agent, ME, PEER, id).await.unwrap().unwrap();
    assert_eq!(
        (resent.id.as_str(), resent.status),
        (id, MessageStatus::Sent)
    );
    assert!(
        engine.resend(&agent, ME, PEER, id).await.is_err(),
        "a sent message was resent"
    );
}

#[tokio::test]
async fn the_peers_receipts_move_my_message_up_the_ladder() {
    let agent = FakeAgent::with_window();
    let engine = Engine::default();
    let sent = engine
        .send(&agent, ME, PEER, outgoing("hi"))
        .await
        .unwrap()
        .unwrap();
    engine
        .delivered(
            &agent,
            from_peer(command::ack(AckKind::Read, &sent.id, 1.0)),
        )
        .await;
    engine
        .delivered(
            &agent,
            from_peer(command::ack(AckKind::Delivered, &sent.id, 2.0)),
        )
        .await;
    assert_eq!(
        status_of(&agent, &sent.id).await,
        Some(MessageStatus::Read),
        "a late delivered undid read"
    );
}

#[tokio::test]
async fn marking_read_sends_receipts_only_when_the_account_does() {
    let agent = FakeAgent::with_window();
    let engine = Engine::default();
    engine.delivered(&agent, from_peer(chat("m1", "a"))).await;
    let prefs = AccountPreferences {
        send_read_receipts: false,
        ..AccountPreferences::UI_DEFAULTS
    };
    save_preferences(&agent, ME, &prefs).await.unwrap();
    engine.mark_read(&agent, ME, PEER).await.unwrap();
    assert_eq!(status_of(&agent, "m1").await, Some(MessageStatus::Read));
    assert!(!agent.sent_commands().iter().any(|c| matches!(
        c,
        Inbound::Ack {
            kind: AckKind::Read,
            ..
        }
    )));
}

#[tokio::test]
async fn ephemeral_traffic_needs_no_window_and_window_traffic_waits_for_one() {
    let agent = FakeAgent::default();
    let engine = Engine::default();
    let typing = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/p2p_commands/typing.cbor"
    ))
    .unwrap();
    assert!(engine.delivered(&agent, from_peer(typing)).await);
    assert!(
        !engine
            .delivered(&agent, from_peer(b"a revfs or file frame".to_vec()))
            .await
    );
    agent.windows.store(true, Ordering::SeqCst);
    assert!(
        engine
            .delivered(&agent, from_peer(b"a revfs or file frame".to_vec()))
            .await
    );
}

#[path = "engine_tests_more.rs"]
mod more;

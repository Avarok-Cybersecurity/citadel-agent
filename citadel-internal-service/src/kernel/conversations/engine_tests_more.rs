use super::super::command::{self, Envelope, Inbound};
use super::super::engine::{store, Engine};
use super::super::engine_fake::*;
use citadel_internal_service_types::MessageType;
use std::sync::Arc;

/// The point of a single writer. Inbound messages and sends from two windows,
/// all at once, against a store that interleaves at every await: every one of
/// them must be on disk afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writers_lose_nothing() {
    let agent = Arc::new(FakeAgent::with_window());
    let engine = Arc::new(Engine::default());
    let mut tasks = Vec::new();
    for i in 0..20 {
        let (a, e) = (agent.clone(), engine.clone());
        tasks.push(tokio::spawn(async move {
            e.delivered(&*a, from_peer(chat(&format!("in-{i}"), "x")))
                .await;
        }));
        let (a, e) = (agent.clone(), engine.clone());
        tasks.push(tokio::spawn(async move {
            e.send(&*a, ME, PEER, outgoing("y")).await.unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let s = store(&*agent, ME);
    let meta = s.load_metadata(PEER).await.unwrap().unwrap();
    let mut stored = 0;
    for page in 0..=meta.latest_page {
        stored += s
            .load_page(PEER, page)
            .await
            .unwrap()
            .map_or(0, |p| p.messages.len());
    }
    assert_eq!(stored, 40, "a concurrent write was lost");
    assert_eq!(meta.total_message_count, 40.0);
}

#[tokio::test]
async fn the_agent_answers_a_checkstate_for_the_account() {
    let agent = FakeAgent::default();
    let check = command::messaging_layer(
        super::super::cbor::Value::object(vec![(
            "type",
            super::super::cbor::Value::text("CheckState"),
        )]),
        &Envelope {
            sender_cid: PEER,
            recipient_cid: ME,
            message_id: "c".into(),
            index: 0.0,
            reply_to: None,
            mentions: None,
            attachments: None,
            message_type: MessageType::Text,
            document_id: None,
            document_title: None,
        },
    );
    assert!(Engine::default().delivered(&agent, from_peer(check)).await);
    assert_eq!(
        agent.sent_commands(),
        vec![Inbound::Ephemeral],
        "no CheckStateResponse went back"
    );
}

#[tokio::test]
async fn only_the_sender_may_edit_and_edits_are_announced() {
    let agent = FakeAgent::with_window();
    let engine = Engine::default();
    let mine = engine
        .send(&agent, ME, PEER, outgoing("mine"))
        .await
        .unwrap()
        .unwrap();
    // The peer tries to edit MY message: ignored, and still handled.
    let forged = command::messaging_layer(
        command::edit_layer(&mine.id, "hacked", 9.0),
        &Envelope {
            sender_cid: PEER,
            recipient_cid: ME,
            ..super::super::envelope::raw_envelope(&agent, ME, PEER).await
        },
    );
    assert!(engine.delivered(&agent, from_peer(forged)).await);
    let (_, page, i) = store(&agent, ME)
        .find(PEER, &mine.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page.messages[i].content, "mine");

    let edited = engine
        .revise_own(&agent, ME, PEER, &mine.id, Some("mine, fixed".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(edited.content, "mine, fixed");
    assert!(
        matches!(agent.sent_commands().last(), Some(Inbound::Edit { contents, .. }) if contents == "mine, fixed")
    );
}

/// A peer naming the account itself as the sender must not have its words
/// rendered as "You": the transport peer is the sender.
#[tokio::test]
async fn a_forged_sender_is_replaced_by_the_transport_peer() {
    let agent = FakeAgent::with_window();
    let forged = command::messaging_layer(
        command::message_layer("I am you", 1.0),
        &Envelope {
            sender_cid: ME,
            recipient_cid: ME,
            message_id: "forged".into(),
            index: 1.0,
            reply_to: None,
            mentions: None,
            attachments: None,
            message_type: MessageType::Text,
            document_id: None,
            document_title: None,
        },
    );
    assert!(Engine::default().delivered(&agent, from_peer(forged)).await);
    let (_, page, i) = store(&agent, ME)
        .find(PEER, "forged")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (page.messages[i].sender_cid, page.messages[i].recipient_cid),
        (PEER, ME)
    );
}

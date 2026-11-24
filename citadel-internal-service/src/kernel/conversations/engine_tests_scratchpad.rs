//! The scratchpad: an account's conversation with itself, stored like any other and never sent.

use super::engine::{has_peer, store, Engine};
use super::engine_fake::*;
use citadel_internal_service_types::{ConversationEventKind, MessageStatus};

/// Every action a window can take in a conversation, in the scratchpad: each is stored and
/// announced, and not one byte goes to a peer.
#[tokio::test]
async fn nothing_in_the_scratchpad_is_ever_sent() {
    let agent = FakeAgent::with_window();
    let engine = Engine::default();

    let note = engine
        .send(&agent, ME, ME, outgoing("buy milk"))
        .await
        .expect("a note is stored")
        .expect("and comes back");
    engine
        .revise_own(&agent, ME, ME, &note.id, Some("buy oat milk".into()))
        .await
        .expect("a note can be edited");
    engine
        .react(&agent, ME, ME, &note.id, "✅".into(), true)
        .await
        .expect("a note can be marked");
    engine
        .mark_read(&agent, ME, ME)
        .await
        .expect("reading it sends no receipt");
    let second = engine
        .send(&agent, ME, ME, outgoing("draft"))
        .await
        .unwrap()
        .unwrap();
    engine
        .revise_own(&agent, ME, ME, &second.id, None)
        .await
        .expect("a note can be deleted");

    assert!(
        agent.sent.lock().is_empty(),
        "the scratchpad sent {:?}",
        agent.sent_commands()
    );
    let (_, page, index) = store(&agent, ME).find(ME, &note.id).await.unwrap().unwrap();
    let stored = &page.messages[index];
    assert_eq!(
        (stored.content.as_str(), stored.status),
        ("buy oat milk", MessageStatus::Sent)
    );
    let kinds: Vec<ConversationEventKind> = agent.events().into_iter().map(|(k, _, _)| k).collect();
    assert!(kinds.contains(&ConversationEventKind::Appended));
    assert!(kinds.contains(&ConversationEventKind::Removed));
}

/// The rule is exactly "the peer is the account itself": a conversation with anyone else
/// still reaches them, so the scratchpad's silence cannot spread to real chats.
#[tokio::test]
async fn a_conversation_with_someone_else_still_reaches_them() {
    assert!(!has_peer(ME, ME));
    assert!(has_peer(ME, PEER));
    let agent = FakeAgent::with_window();
    Engine::default()
        .send(&agent, ME, PEER, outgoing("hello bob"))
        .await
        .unwrap();
    assert_eq!(
        agent.sent.lock().len(),
        1,
        "a message to a peer was not sent"
    );
}

/// A note is the account's own: it is never counted unread.
#[tokio::test]
async fn a_note_is_never_unread() {
    let agent = FakeAgent::with_window();
    Engine::default()
        .send(&agent, ME, ME, outgoing("remember"))
        .await
        .unwrap();
    let meta = store(&agent, ME).load_metadata(ME).await.unwrap().unwrap();
    assert_eq!(meta.unread_count, 0.0);
}

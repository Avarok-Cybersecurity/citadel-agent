//! The agent keeps a hosted account's conversations, over the real SDK.
//!
//! Each agent hosts its account's ILM and conversation store. Where Bob sends a
//! raw chat command, it is the bytes a web UI's encoder writes
//! (tests/fixtures/p2p_commands, written by cbor-x).
use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::agent_ilm::*;
    use crate::common::group::recv_until;
    use crate::common::{open_localhost_connection, setup_log};
    use citadel_internal_service_types::{
        ConfigCommand, ConversationEventKind, InternalServiceRequest, InternalServiceResponse,
        MessageStatus,
    };
    use tokio::sync::mpsc::UnboundedReceiver;
    use uuid::Uuid;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/p2p_commands/{name}.cbor",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    const FIXTURE_ID: &str = "0b5f2b0e-3c8e-4b7a-9f00-2a4c7e1d9a11";

    async fn page(
        window: &mut (
            tokio::sync::mpsc::UnboundedSender<InternalServiceRequest>,
            UnboundedReceiver<InternalServiceResponse>,
        ),
        cid: u64,
        peer: u64,
    ) -> Vec<(String, MessageStatus)> {
        let request_id = Uuid::new_v4();
        window
            .0
            .send(InternalServiceRequest::ConversationPage {
                request_id,
                cid,
                peer_cid: peer,
                page: None,
            })
            .unwrap();
        match recv_until(&mut window.1, "the page", |r| {
            r.request_id() == Some(&request_id)
        })
        .await
        {
            InternalServiceResponse::ConversationPageResponse(p) => p
                .page
                .map(|p| p.messages.into_iter().map(|m| (m.id, m.status)).collect())
                .unwrap_or_default(),
            other => panic!("no page: {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_arriving_chat_message_is_stored_and_announced() {
        setup_log();
        let mut world = hosted_pair().await;
        let (alice, bob) = (world.alice.2, world.bob.2);
        sends(&mut world.bob, alice, &fixture("message")).await;

        let event = recv_until(&mut world.alice.1, "the event", |r| {
            matches!(r, InternalServiceResponse::ConversationEvent(_))
        })
        .await;
        let InternalServiceResponse::ConversationEvent(event) = event else {
            unreachable!()
        };
        assert_eq!(
            (event.kind, event.peer_cid, event.preview.as_str()),
            (ConversationEventKind::Appended, bob, "hello")
        );
        let message = event.message.expect("the message");
        assert_eq!(
            (message.sender_cid, message.recipient_cid, message.status),
            (bob, alice, MessageStatus::Delivered)
        );

        let mut window = (world.alice.0.clone(), world.alice.1);
        assert_eq!(
            page(&mut window, alice, bob).await,
            [(FIXTURE_ID.to_string(), MessageStatus::Delivered)]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_message_sent_from_a_window_is_numbered_stored_and_delivered() {
        setup_log();
        let mut world = hosted_pair().await;
        let (alice, bob) = (world.alice.2, world.bob.2);
        let (request_id, request) = conversation_send(alice, bob, "from the agent");
        world.alice.0.send(request).unwrap();
        // The window's own bubble first, named by its request: the event is
        // announced before ILM has the message, and the answer after.
        let shown = recv_until(&mut world.alice.1, "the bubble", |r| {
            matches!(r, InternalServiceResponse::ConversationEvent(_))
        })
        .await;
        let InternalServiceResponse::ConversationEvent(shown) = shown else {
            unreachable!()
        };
        assert_eq!(
            (shown.kind, shown.request_id),
            (ConversationEventKind::Appended, Some(request_id))
        );
        let answer = recv_until(&mut world.alice.1, "the send", |r| {
            matches!(r, InternalServiceResponse::ConversationUpdated(_))
                && r.request_id() == Some(&request_id)
        })
        .await;
        let InternalServiceResponse::ConversationUpdated(sent) = answer else {
            panic!("not sent: {answer:?}")
        };
        let sent = sent.message.expect("the message");
        assert_eq!((sent.status, sent.index), (MessageStatus::Sent, 1.0));

        // Bob's agent stores it, and its receipt marks it delivered at Alice.
        let at_bob = event_of(&mut world.bob.1, ConversationEventKind::Appended, |m| {
            m.id == sent.id
        })
        .await;
        assert_eq!(
            (at_bob.content.as_str(), at_bob.sender_cid),
            ("from the agent", alice)
        );
        event_of(&mut world.alice.1, ConversationEventKind::Updated, |m| {
            m.id == sent.id && m.status == MessageStatus::Delivered
        })
        .await;
    }

    /// No window: stored and receipted all the same, and there on attach.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_message_that_arrives_with_no_window_open_is_kept_in_the_store() {
        setup_log();
        let HostedPair {
            alice_addr,
            alice,
            mut bob,
        } = hosted_pair().await;
        let (alice_cid, bob_cid) = (alice.2, bob.2);
        drop(alice);

        let (request_id, request) = conversation_send(bob_cid, alice_cid, "while you were away");
        bob.0.send(request).unwrap();
        let sent = recv_until(&mut bob.1, "the send", |r| {
            matches!(r, InternalServiceResponse::ConversationUpdated(_))
                && r.request_id() == Some(&request_id)
        })
        .await;
        let InternalServiceResponse::ConversationUpdated(sent) = sent else {
            panic!("not sent: {sent:?}")
        };
        let id = sent.message.expect("the message").id;
        event_of(&mut bob.1, ConversationEventKind::Updated, |m| {
            m.id == id && m.status == MessageStatus::Delivered
        })
        .await;

        let mut window = open_localhost_connection(alice_addr).await.unwrap();
        window
            .0
            .send(InternalServiceRequest::ConnectionManagement {
                request_id: Uuid::new_v4(),
                management_command: ConfigCommand::ClaimSession {
                    session_cid: alice_cid,
                    only_if_orphaned: true,
                },
            })
            .unwrap();
        recv_until(&mut window.1, "the claim", |r| {
            matches!(r, InternalServiceResponse::ConnectionManagementSuccess(_))
        })
        .await;
        assert_eq!(
            page(&mut window, alice_cid, bob_cid).await,
            [(id, MessageStatus::Delivered)]
        );
    }

    /// Another connection on the agent -- another account's window, or any
    /// local page -- cannot read this account's conversations by naming it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_connection_not_attached_to_the_session_cannot_read_its_history() {
        setup_log();
        let mut world = hosted_pair().await;
        let (alice, bob) = (world.alice.2, world.bob.2);
        sends(&mut world.bob, alice, &fixture("message")).await;
        recv_until(&mut world.alice.1, "stored", |r| {
            matches!(r, InternalServiceResponse::ConversationEvent(_))
        })
        .await;

        let mut stranger = open_localhost_connection(world.alice_addr).await.unwrap();
        let request_id = Uuid::new_v4();
        stranger
            .0
            .send(InternalServiceRequest::ConversationPage {
                request_id,
                cid: alice,
                peer_cid: bob,
                page: None,
            })
            .unwrap();
        let answer = recv_until(&mut stranger.1, "the refusal", |r| {
            r.request_id() == Some(&request_id)
        })
        .await;
        assert!(
            matches!(answer, InternalServiceResponse::ConversationFailure(_)),
            "a stranger read the conversation: {answer:?}"
        );
    }
}

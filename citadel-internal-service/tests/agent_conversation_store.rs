//! The agent keeps a hosted account's conversations, over the real SDK.
//!
//! Alice's agent hosts her ILM and her conversation store; Bob runs the
//! browser's messenger. Bob's chat commands are the bytes a web UI sends
//! (tests/fixtures/p2p_commands, written by cbor-x).
use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::agent_ilm::*;
    use crate::common::group::recv_until;
    use crate::common::{open_localhost_connection, setup_log};
    use citadel_internal_service_connector::messenger::ACCOUNT_ILM_OPTIONS;
    use citadel_internal_service_types::{
        ConfigCommand, ConversationEventKind, InternalServiceRequest, InternalServiceResponse,
        MessageStatus, MessageType, SecurityLevel,
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

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

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
    async fn an_arriving_chat_message_is_stored_receipted_and_announced() {
        setup_log();
        let mut world = mixed_hosting(ACCOUNT_ILM_OPTIONS).await;
        let (alice, bob) = (world.alice.2, world.bob.cid);
        world
            .bob
            .tx
            .send_message_to(alice, fixture("message"))
            .await
            .expect("bob sends");

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

        // Bob's UI is told "delivered", in its own encoding.
        let receipt = next_message(&mut world.bob.rx, alice).await;
        assert!(
            contains(&receipt, b"MessageAck") && contains(&receipt, FIXTURE_ID.as_bytes()),
            "no delivery receipt"
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
        let mut world = mixed_hosting(ACCOUNT_ILM_OPTIONS).await;
        let (alice, bob) = (world.alice.2, world.bob.cid);
        let request_id = Uuid::new_v4();
        world
            .alice
            .0
            .send(InternalServiceRequest::ConversationSend {
                request_id,
                cid: alice,
                peer_cid: bob,
                content: "from the agent".into(),
                message_type: MessageType::Text,
                reply_to: None,
                mentions: None,
                attachments: None,
                document_id: None,
                document_title: None,
                security_level: SecurityLevel::Standard,
            })
            .unwrap();
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

        let at_bob = next_message(&mut world.bob.rx, alice).await;
        assert!(contains(&at_bob, b"from the agent") && contains(&at_bob, sent.id.as_bytes()));
    }

    /// No window: stored and receipted all the same, and there on attach.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_message_that_arrives_with_no_window_open_is_kept_in_the_store() {
        setup_log();
        let world = mixed_hosting(ACCOUNT_ILM_OPTIONS).await;
        let MixedHosting {
            alice_addr,
            alice,
            mut bob,
        } = world;
        let (alice_cid, bob_cid) = (alice.2, bob.cid);
        drop(alice);

        bob.tx
            .send_message_to(alice_cid, fixture("message"))
            .await
            .expect("bob sends");
        let receipt = next_message(&mut bob.rx, alice_cid).await;
        assert!(
            contains(&receipt, b"MessageAck"),
            "the agent did not receipt it with no window open"
        );

        let mut window = open_localhost_connection(alice_addr).await.unwrap();
        declared(&mut window).await;
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
            [(FIXTURE_ID.to_string(), MessageStatus::Delivered)]
        );
    }

    /// Another connection on the agent -- another account's window, or any
    /// local page -- cannot read this account's conversations by naming it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_connection_not_attached_to_the_session_cannot_read_its_history() {
        setup_log();
        let mut world = mixed_hosting(ACCOUNT_ILM_OPTIONS).await;
        let (alice, bob) = (world.alice.2, world.bob.cid);
        world
            .bob
            .tx
            .send_message_to(alice, fixture("message"))
            .await
            .expect("bob sends");
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

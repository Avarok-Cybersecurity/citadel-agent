//! Native notices over the real SDK (multi-window mw6).
//!
//! Alice's agent hosts her account; Bob runs a browser messenger and sends
//! the bytes a web UI sends. The menu-bar app is a plain localhost connection
//! holding the launch token every test agent is started with.
use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::agent_ilm::*;
    use crate::common::group::recv_until;
    use crate::common::{open_localhost_connection, setup_log, TEST_NOTICE_TOKEN};
    use citadel_internal_service_connector::messenger::ACCOUNT_ILM_OPTIONS;
    use citadel_internal_service_types::{
        AccountPreferences, ConfigCommand, InternalServiceRequest, InternalServiceResponse,
        NativeNotice, NoticeKind, NotificationPreview,
    };
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
    use uuid::Uuid;

    type Conn = (
        UnboundedSender<InternalServiceRequest>,
        UnboundedReceiver<InternalServiceResponse>,
    );

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/p2p_commands/{name}.cbor",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    async fn subscribe(app: &mut Conn, token: &str) -> InternalServiceResponse {
        let request_id = Uuid::new_v4();
        app.0
            .send(InternalServiceRequest::NoticeSubscribe {
                request_id,
                token: token.into(),
            })
            .unwrap();
        recv_until(&mut app.1, "the subscription", |r| {
            r.request_id() == Some(&request_id)
        })
        .await
    }

    async fn next_notice(app: &mut Conn) -> NativeNotice {
        match recv_until(&mut app.1, "a notice", |r| {
            matches!(r, InternalServiceResponse::NativeNotice(_))
        })
        .await
        {
            InternalServiceResponse::NativeNotice(n) => *n,
            _ => unreachable!(),
        }
    }

    async fn answered(
        tx: &UnboundedSender<InternalServiceRequest>,
        rx: &mut UnboundedReceiver<InternalServiceResponse>,
        request: InternalServiceRequest,
    ) -> InternalServiceResponse {
        let id = *request.request_id().expect("a request id");
        tx.send(request).unwrap();
        recv_until(rx, "the answer", |r| r.request_id() == Some(&id)).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn only_the_launch_token_subscribes() {
        setup_log();
        let world = mixed_hosting(ACCOUNT_ILM_OPTIONS).await;
        let mut page = open_localhost_connection(world.alice_addr).await.unwrap();
        assert!(matches!(
            subscribe(&mut page, "a guess").await,
            InternalServiceResponse::NoticeFailure(_)
        ));
        let mut app = open_localhost_connection(world.alice_addr).await.unwrap();
        match subscribe(&mut app, TEST_NOTICE_TOKEN).await {
            InternalServiceResponse::NoticeRows(rows) => {
                assert!(
                    rows.rows.iter().any(|r| r.cid == world.alice.2 && !r.muted),
                    "{rows:?}"
                );
            }
            other => panic!("not subscribed: {other:?}"),
        }
    }

    /// No window focused: the message is noticed, naming the sender and, by
    /// default, not the text. With the conversation in front of the user it
    /// is not; with previews on, the text shows; muted, nothing does.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_message_is_noticed_unless_shown_or_muted() {
        setup_log();
        let mut world = mixed_hosting(ACCOUNT_ILM_OPTIONS).await;
        let (alice, bob) = (world.alice.2, world.bob.cid);
        let mut app = open_localhost_connection(world.alice_addr).await.unwrap();
        subscribe(&mut app, TEST_NOTICE_TOKEN).await;

        world
            .bob
            .tx
            .send_message_to(alice, fixture("message"))
            .await
            .expect("bob sends");
        let first = next_notice(&mut app).await;
        assert_eq!(
            (first.kind, first.cid, first.body.as_str()),
            (NoticeKind::Message, alice, "New message")
        );
        assert_eq!(first.target.open, format!("conversation:{bob}"));
        assert!(!format!("{:?}", first.target).contains("hello"));

        // Alice reads Bob's conversation; then turns previews on and looks away.
        let focus = |focused| InternalServiceRequest::ConnectionManagement {
            request_id: Uuid::new_v4(),
            management_command: ConfigCommand::ReportFocus {
                session_cid: alice,
                peer_cid: Some(bob),
                focused,
            },
        };
        answered(&world.alice.0, &mut world.alice.1, focus(true)).await;
        world
            .bob
            .tx
            .send_message_to(alice, fixture("message-full"))
            .await
            .expect("bob sends");
        recv_until(&mut world.alice.1, "the second message", |r| {
            matches!(r, InternalServiceResponse::ConversationEvent(e) if e.message_id.as_deref() == Some("id-full"))
        })
        .await;
        let prefs = AccountPreferences {
            notification_preview: NotificationPreview::Text,
            ..AccountPreferences::UI_DEFAULTS
        };
        let set = InternalServiceRequest::SetAccountPreferences {
            request_id: Uuid::new_v4(),
            cid: alice,
            preferences: Box::new(prefs),
        };
        answered(&world.alice.0, &mut world.alice.1, set).await;
        answered(&world.alice.0, &mut world.alice.1, focus(false)).await;
        world
            .bob
            .tx
            .send_message_to(alice, fixture("message-long"))
            .await
            .expect("bob sends");
        // The next notice is the third message's: the second was on screen.
        let third = next_notice(&mut app).await;
        assert!(
            third.body.starts_with("xxxxxxxx"),
            "{:?}",
            third.body.get(..20)
        );

        let mute = InternalServiceRequest::NoticeSetMuted {
            request_id: Uuid::new_v4(),
            token: TEST_NOTICE_TOKEN.into(),
            cid: alice,
            muted: true,
        };
        match answered(&app.0, &mut app.1, mute).await {
            InternalServiceResponse::NoticeRows(rows) => {
                assert!(rows.rows.iter().any(|r| r.cid == alice && r.muted))
            }
            other => panic!("not muted: {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_window_may_report_focus_only_for_its_own_session() {
        setup_log();
        let world = mixed_hosting(ACCOUNT_ILM_OPTIONS).await;
        let mut stranger = open_localhost_connection(world.alice_addr).await.unwrap();
        let report = InternalServiceRequest::ConnectionManagement {
            request_id: Uuid::new_v4(),
            management_command: ConfigCommand::ReportFocus {
                session_cid: world.alice.2,
                peer_cid: None,
                focused: true,
            },
        };
        assert!(matches!(
            answered(&stranger.0, &mut stranger.1, report).await,
            InternalServiceResponse::ConnectionManagementFailure(_)
        ));
    }
}

//! A page older than agent hosting never runs beside a hosted account.
//!
//! "Older page" = a connection that never declared `agent_ilm`: it would run
//! its own ILM and write the conversation pages itself. It is refused at every
//! door into a hosted session, pushed out of one it held when hosting starts,
//! and nobody may write a hosted account's conversation records but the agent.
use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::agent_ilm::*;
    use crate::common::group::recv_until;
    use crate::common::multi_window::{attach, next_role, open_window, password, ALICE_PASSWORD};
    use crate::common::{open_localhost_connection, setup_log, two_sessions_on_one_service_at};
    use citadel_internal_service_connector::messenger::ACCOUNT_ILM_OPTIONS;
    use citadel_internal_service_types::{
        ConfigCommand, InternalServiceRequest, InternalServiceResponse, SessionRole,
    };
    use citadel_sdk::prelude::*;
    use uuid::Uuid;

    const RELOAD: &str = "Reload it to continue";

    fn connect_as(username: &str) -> InternalServiceRequest {
        InternalServiceRequest::Connect {
            request_id: Uuid::new_v4(),
            username: username.to_string(),
            password: SecBuffer::from(ALICE_PASSWORD.as_bytes().to_vec()),
            connect_mode: ConnectMode::Standard { force_login: true },
            udp_mode: Default::default(),
            keep_alive_timeout: None,
            session_security_settings: SessionSecuritySettingsBuilder::default().build().unwrap(),
            server_password: None,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_older_page_cannot_enter_a_hosted_session() {
        setup_log();
        let world = mixed_hosting(ACCOUNT_ILM_OPTIONS).await;
        let MixedHosting {
            alice_addr,
            alice,
            bob: _bob,
        } = world;
        let alice_cid = alice.2;
        drop(alice); // orphaned, still hosted

        let mut older = open_localhost_connection(alice_addr).await.unwrap();
        older
            .0
            .send(InternalServiceRequest::ConnectionManagement {
                request_id: Uuid::new_v4(),
                management_command: ConfigCommand::ClaimSession {
                    session_cid: alice_cid,
                    only_if_orphaned: true,
                },
            })
            .unwrap();
        match recv_until(&mut older.1, "the claim", |r| {
            matches!(
                r,
                InternalServiceResponse::ConnectionManagementFailure(_)
                    | InternalServiceResponse::ConnectionManagementSuccess(_)
            )
        })
        .await
        {
            InternalServiceResponse::ConnectionManagementFailure(f) => {
                assert!(f.error.contains(RELOAD), "{}", f.error)
            }
            other => panic!("an older page claimed a hosted session: {other:?}"),
        }

        // The same page signing in with the right password: refused too, never taken over.
        let username = crate::common::agent_ilm::username_of(alice_addr, alice_cid).await;
        older.0.send(connect_as(&username)).unwrap();
        match recv_until(&mut older.1, "the sign-in", |r| {
            matches!(
                r,
                InternalServiceResponse::ConnectFailure(_)
                    | InternalServiceResponse::ConnectSuccess(_)
                    | InternalServiceResponse::SessionAlreadyActive(_)
            )
        })
        .await
        {
            InternalServiceResponse::ConnectFailure(f) => {
                assert!(f.message.contains(RELOAD), "{}", f.message)
            }
            other => panic!("an older page signed in to a hosted session: {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hosting_pushes_an_older_page_out_of_the_session() {
        setup_log();
        let (addr, mut older, _bob) = two_sessions_on_one_service_at("guard.displace")
            .await
            .unwrap();
        let cid = older.2;
        let mut newer = open_window(addr).await.unwrap();
        declared(&mut newer).await;
        attach(&mut newer, cid, password(ALICE_PASSWORD))
            .await
            .expect("the newer page attaches");

        // First told it has company (the attach), then that it is out (hosting began).
        assert_eq!(
            next_role(&mut older.1, cid).await,
            (SessionRole::Primary, 2)
        );
        assert_eq!(
            next_role(&mut older.1, cid).await,
            (SessionRole::Detached, 0)
        );
        let probe = Uuid::new_v4();
        older
            .0
            .send(InternalServiceRequest::LocalDBSetKV {
                request_id: probe,
                cid,
                peer_cid: None,
                key: "inbound_messages-x".into(),
                value: vec![1],
            })
            .unwrap();
        let answer = recv_until(&mut older.1, "the refused write", |r| {
            r.request_id() == Some(&probe)
        })
        .await;
        assert!(
            answer.is_error(),
            "a displaced older page could still write: {answer:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn only_the_agent_writes_a_hosted_accounts_conversation_records() {
        setup_log();
        let mut world = mixed_hosting(ACCOUNT_ILM_OPTIONS).await;
        let (alice, bob) = (world.alice.2, world.bob.cid);
        let probe = Uuid::new_v4();
        world
            .alice
            .0
            .send(InternalServiceRequest::LocalDBSetKV {
                request_id: probe,
                cid: 0,
                peer_cid: None,
                key: format!("msgs_with_peer_{alice}_with_{bob}_metadata"),
                value: b"{}".to_vec(),
            })
            .unwrap();
        let answer = recv_until(&mut world.alice.1, "the write", |r| {
            r.request_id() == Some(&probe)
        })
        .await;
        assert!(
            answer.is_error(),
            "a window wrote the agent's conversation record: {answer:?}"
        );
    }
}

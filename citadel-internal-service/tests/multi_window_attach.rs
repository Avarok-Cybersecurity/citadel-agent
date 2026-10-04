//! Who may join a live session, and what an older UI's takeover still does.
//!
//! Joining a session another window holds requires the proof today's takeover
//! requires: the password (or the token a password attach returned). Without it
//! nothing changes -- asserted as the consequence, that the refused window
//! receives none of the session's traffic, not only as a refusal message.
use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::group::recv_until;
    use crate::common::multi_window::*;
    use citadel_internal_service_types::{
        AttachProof, InternalServiceRequest, InternalServiceResponse, SessionRole,
    };
    use citadel_sdk::prelude::*;
    use std::error::Error;
    use std::time::Duration;
    use uuid::Uuid;

    fn carries_alices_traffic(responses: &[InternalServiceResponse], alice: u64) -> bool {
        responses.iter().any(|r| match r {
            InternalServiceResponse::MessageNotification(n) => n.cid == alice,
            InternalServiceResponse::SessionRoleNotification(n) => n.cid == alice,
            _ => false,
        })
    }

    #[tokio::test]
    async fn an_attach_without_the_password_is_refused() -> Result<(), Box<dyn Error>> {
        crate::common::setup_log();
        let mut accounts = two_connected_accounts("mw.noproof").await?;
        let alice = accounts.alice.2;
        let mut intruder = open_window(accounts.addr).await?;

        let refused = attach(&mut intruder, alice, password("not her password")).await;
        assert_eq!(
            refused,
            Err("The password does not match this session".to_string())
        );
        let forged = attach(&mut intruder, alice, AttachProof::Token(vec![7; 32])).await;
        assert!(
            forged.is_err(),
            "a token the session never issued was accepted"
        );

        // The consequence: Alice's next message reaches her window, and the
        // refused one hears nothing of it.
        let body = bob_messages_alice(&accounts.bob, alice, "private");
        receives_message(&mut accounts.alice.1, "alice's window", alice, &body).await;
        let heard = drain(&mut intruder.1, Duration::from_millis(500)).await;
        assert!(
            !carries_alices_traffic(&heard, alice),
            "a refused attach still received the session's traffic: {heard:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn an_attach_with_the_password_succeeds_and_returns_a_token() -> Result<(), Box<dyn Error>>
    {
        crate::common::setup_log();
        let mut accounts = two_connected_accounts("mw.proof").await?;
        let alice = accounts.alice.2;
        let mut second = open_window(accounts.addr).await?;

        let (role, token) = attach(&mut second, alice, password(ALICE_PASSWORD)).await?;
        assert_eq!(role, SessionRole::Secondary);
        assert_eq!(token.len(), 32);
        // The window that already had the session is told it has company.
        assert_eq!(
            next_role(&mut accounts.alice.1, alice).await,
            (SessionRole::Primary, 2)
        );

        // The token admits a further window without the password, while the
        // session lives -- this is how a joined browser is not asked again.
        let mut third = open_window(accounts.addr).await?;
        let (role, _) = attach(&mut third, alice, AttachProof::Token(token)).await?;
        assert_eq!(role, SessionRole::Secondary);
        let body = bob_messages_alice(&accounts.bob, alice, "to all three");
        receives_message(&mut third.1, "the token window", alice, &body).await;
        Ok(())
    }

    /// A UI that predates AttachSession signs in with its password and takes
    /// the session over, exactly as before. The windows it displaced are now
    /// told, and stop receiving.
    #[tokio::test]
    async fn a_legacy_sign_in_still_takes_the_session_over() -> Result<(), Box<dyn Error>> {
        crate::common::setup_log();
        let mut accounts = two_connected_accounts("mw.legacy").await?;
        let alice = accounts.alice.2;
        let mut second = open_window(accounts.addr).await?;
        attach(&mut second, alice, password(ALICE_PASSWORD)).await?;
        assert_eq!(
            next_role(&mut accounts.alice.1, alice).await,
            (SessionRole::Primary, 2)
        );
        let mut legacy = open_window(accounts.addr).await?;

        legacy.0.send(InternalServiceRequest::Connect {
            request_id: Uuid::new_v4(),
            username: "mw.legacy.0".to_string(),
            password: Some(SecBuffer::from(ALICE_PASSWORD.as_bytes().to_vec())),
            security_key: false,
            recovery_code: None,
            connect_mode: ConnectMode::Standard { force_login: true },
            udp_mode: Default::default(),
            keep_alive_timeout: None,
            session_security_settings: SessionSecuritySettingsBuilder::default().build()?,
            server_password: None,
            admission_token: None,
        })?;
        let answer = recv_until(&mut legacy.1, "the takeover's answer", |r| {
            matches!(r, InternalServiceResponse::SessionAlreadyActive(_))
        })
        .await;
        let InternalServiceResponse::SessionAlreadyActive(active) = answer else {
            unreachable!()
        };
        assert_eq!(active.cid, alice);

        assert_eq!(
            next_role(&mut accounts.alice.1, alice).await,
            (SessionRole::Detached, 0)
        );
        assert_eq!(
            next_role(&mut second.1, alice).await,
            (SessionRole::Detached, 0)
        );
        let body = bob_messages_alice(&accounts.bob, alice, "who has it now");
        receives_message(&mut legacy.1, "the window that took it", alice, &body).await;
        let heard = drain(&mut second.1, Duration::from_millis(500)).await;
        assert!(
            !carries_alices_traffic(&heard, alice),
            "a displaced window still receives"
        );
        Ok(())
    }
}

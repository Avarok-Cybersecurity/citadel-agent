//! One session, two windows: what each of them receives.
//!
//! Alice's session is opened by one localhost connection (her first window) and
//! a second connection attaches to it with her password. Both are then real
//! subscribers: everything session-scoped reaches both, a request's own answer
//! reaches only the window that asked, one window closing leaves the other
//! working, and a logout from either ends the session for both.
use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::group::recv_until;
    use crate::common::multi_window::*;
    use citadel_internal_service_types::{
        InternalServiceRequest, InternalServiceResponse, SessionRole,
    };
    use std::error::Error;
    use std::time::Duration;
    use uuid::Uuid;

    async fn alice_in_two_windows(tag: &str) -> Result<(TwoAccounts, Window), Box<dyn Error>> {
        crate::common::setup_log();
        let accounts = two_connected_accounts(tag).await?;
        let mut second = open_window(accounts.addr).await?;
        let (role, _token) = attach(&mut second, accounts.alice.2, password(ALICE_PASSWORD))
            .await
            .map_err(|e| format!("the attach was refused: {e}"))?;
        assert_eq!(role, SessionRole::Secondary);
        Ok((accounts, second))
    }

    #[tokio::test]
    async fn a_message_reaches_both_windows() -> Result<(), Box<dyn Error>> {
        let (mut accounts, mut second) = alice_in_two_windows("mw.fanout").await?;
        let alice = accounts.alice.2;
        let body = bob_messages_alice(&accounts.bob, alice, "hello from bob");
        receives_message(&mut accounts.alice.1, "the first window", alice, &body).await;
        receives_message(&mut second.1, "the second window", alice, &body).await;
        Ok(())
    }

    /// The answer to the second window's request carries its request_id and
    /// goes to it alone. Proven against the first window's ORDERED stream: a
    /// message sent after the answer must arrive there without the answer
    /// having arrived first.
    #[tokio::test]
    async fn a_response_reaches_only_the_window_that_asked() -> Result<(), Box<dyn Error>> {
        let (mut accounts, mut second) = alice_in_two_windows("mw.response").await?;
        let alice = accounts.alice.2;
        let request_id = Uuid::new_v4();
        second.0.send(InternalServiceRequest::LocalDBSetKV {
            request_id,
            cid: alice,
            peer_cid: None,
            key: "multi-window-probe".to_string(),
            value: vec![1, 2, 3],
        })?;
        let answer = recv_until(&mut second.1, "the set's answer", |r| {
            r.request_id() == Some(&request_id)
        })
        .await;
        assert!(
            matches!(answer, InternalServiceResponse::LocalDBSetKVSuccess(_)),
            "the window that asked got {answer:?}"
        );

        let body = bob_messages_alice(&accounts.bob, alice, "after the answer");
        let first_saw = recv_until(&mut accounts.alice.1, "the later message", |r| {
            assert_ne!(
                r.request_id(),
                Some(&request_id),
                "the second window's answer reached the first window"
            );
            matches!(r, InternalServiceResponse::MessageNotification(n) if n.message == body)
        })
        .await;
        assert!(matches!(
            first_saw,
            InternalServiceResponse::MessageNotification(_)
        ));
        Ok(())
    }

    /// The first window closing leaves the second attached, promoted, and
    /// still receiving.
    #[tokio::test]
    async fn one_window_closing_leaves_the_other_receiving() -> Result<(), Box<dyn Error>> {
        let (accounts, mut second) = alice_in_two_windows("mw.drop").await?;
        let alice = accounts.alice.2;
        let TwoAccounts {
            alice: first, bob, ..
        } = accounts;
        drop(first);

        assert_eq!(
            next_role(&mut second.1, alice).await,
            (SessionRole::Primary, 1)
        );
        let body = bob_messages_alice(&bob, alice, "still there?");
        receives_message(&mut second.1, "the remaining window", alice, &body).await;
        Ok(())
    }

    /// Logout from either window ends the session for both, and both are told.
    #[tokio::test]
    async fn a_logout_reaches_both_windows() -> Result<(), Box<dyn Error>> {
        let (mut accounts, mut second) = alice_in_two_windows("mw.logout").await?;
        let alice = accounts.alice.2;
        let request_id = Uuid::new_v4();
        second.0.send(InternalServiceRequest::Disconnect {
            request_id,
            cid: alice,
        })?;

        let mine = recv_until(&mut second.1, "the logout's answer", |r| {
            matches!(r, InternalServiceResponse::DisconnectNotification(n) if n.cid == alice && n.peer_cid.is_none())
        })
        .await;
        assert_eq!(mine.request_id(), Some(&request_id));

        let theirs = recv_until(&mut accounts.alice.1, "the other window's notice", |r| {
            matches!(r, InternalServiceResponse::DisconnectNotification(n) if n.cid == alice && n.peer_cid.is_none())
        })
        .await;
        assert_eq!(
            theirs.request_id(),
            None,
            "the notice should not claim to answer a request"
        );

        // And it is really gone: neither window can act on it any more.
        let quiet = drain(&mut second.1, Duration::from_millis(200)).await;
        drop(quiet);
        let probe = Uuid::new_v4();
        accounts
            .alice
            .0
            .send(InternalServiceRequest::LocalDBSetKV {
                request_id: probe,
                cid: alice,
                peer_cid: None,
                key: "after-logout".to_string(),
                value: vec![0],
            })?;
        let refused = recv_until(&mut accounts.alice.1, "the refused write", |r| {
            r.request_id() == Some(&probe)
        })
        .await;
        assert!(
            refused.is_error(),
            "a write after logout was accepted: {refused:?}"
        );
        Ok(())
    }
}

//! Deleting an account from one window reaches every window attached to it,
//! as a logout does: the others must leave a workspace that no longer exists.
use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::group::recv_until;
    use crate::common::multi_window::*;
    use citadel_internal_service_types::{
        InternalServiceRequest, InternalServiceResponse, SessionRole,
    };
    use std::error::Error;
    use uuid::Uuid;

    #[tokio::test]
    async fn a_deregister_reaches_both_windows() -> Result<(), Box<dyn Error>> {
        crate::common::setup_log();
        let mut accounts = two_connected_accounts("mw.deregister").await?;
        let mut second = open_window(accounts.addr).await?;
        let (role, _token) = attach(&mut second, accounts.alice.2, password(ALICE_PASSWORD))
            .await
            .map_err(|e| format!("the attach was refused: {e}"))?;
        assert_eq!(role, SessionRole::Secondary);
        let alice = accounts.alice.2;
        let request_id = Uuid::new_v4();
        second.0.send(InternalServiceRequest::Deregister {
            request_id,
            cid: alice,
        })?;

        let mine = recv_until(
            &mut second.1,
            "the deregister's answer",
            |r| matches!(r, InternalServiceResponse::DeregisterSuccess(n) if n.cid == alice),
        )
        .await;
        assert_eq!(mine.request_id(), Some(&request_id));

        let theirs = recv_until(
            &mut accounts.alice.1,
            "the other window's notice",
            |r| matches!(r, InternalServiceResponse::DeregisterSuccess(n) if n.cid == alice),
        )
        .await;
        assert_eq!(
            theirs.request_id(),
            None,
            "the notice should not claim to answer a request"
        );
        Ok(())
    }
}

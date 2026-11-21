//! An account signed in to the agent with no window showing it is still reachable.
//!
//! A peer's PeerConnect completes only when somebody answers it. Only a window
//! showing that exact account used to, so an account left signed in on the
//! landing page, behind another account, or with every window closed never
//! answered, and the initiator timed out waiting for the channel, retrying for
//! ever (seen live 2026-10-02: one Mac could not reach the other for an hour).
//! The agent now answers for an account it hosts, by the UI's rules.
//!
//! Over the real SDK: two agents on one in-process server, registered to each
//! other. Both are hosted, as every signed-in account is; `B`'s window never
//! answers anything.
use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::agent_ilm::{pause, require_level};
    use crate::common::setup_log;
    use crate::common::turn_harness::{next_matching, two_registered_agents};
    use crate::common::PeerReturnHandle;
    use citadel_internal_service_types::{
        ChatSecurityLevel, InternalServiceRequest, InternalServiceResponse,
    };
    use citadel_sdk::prelude::*;
    use std::error::Error;
    use std::time::Duration;
    use tokio::sync::mpsc::UnboundedReceiver;
    use uuid::Uuid;

    /// Longer than any answer takes, shorter than the SDK's 60s channel wait, so
    /// an unanswered offer fails here rather than as a timeout from the SDK.
    const ANSWERED_WITHIN: Duration = Duration::from_secs(30);

    fn peer_connect(cid: u64, peer_cid: u64, level: SecurityLevel) -> InternalServiceRequest {
        InternalServiceRequest::PeerConnect {
            request_id: Uuid::new_v4(),
            cid,
            peer_cid,
            udp_mode: UdpMode::Disabled,
            session_security_settings: SessionSecuritySettingsBuilder::default()
                .with_security_level(level)
                .build()
                .unwrap(),
            peer_session_password: None,
            turn: None,
        }
    }

    /// What the initiator's PeerConnect came to: Ok on a channel, Err on a refusal.
    async fn outcome(
        rx: &mut UnboundedReceiver<InternalServiceResponse>,
        peer: u64,
    ) -> Result<(), String> {
        tokio::time::timeout(ANSWERED_WITHIN, async {
            loop {
                match rx.recv().await.expect("initiator's window closed") {
                    InternalServiceResponse::PeerConnectSuccess(s) if s.peer_cid == peer => {
                        return Ok(())
                    }
                    InternalServiceResponse::PeerConnectFailure(f) => return Err(f.message),
                    _ => continue,
                }
            }
        })
        .await
        .expect("the offer was never answered: nobody accepted or declined it")
    }

    async fn answered_by_agent(rx: &mut UnboundedReceiver<InternalServiceResponse>) -> bool {
        next_matching(rx, "PeerConnectNotification", |r| match r {
            InternalServiceResponse::PeerConnectNotification(n) => Some(n.answered_by_agent),
            _ => None,
        })
        .await
    }

    fn split(agents: &mut [PeerReturnHandle]) -> (&mut PeerReturnHandle, &mut PeerReturnHandle) {
        let (a, b) = agents.split_at_mut(1);
        (&mut a[0], &mut b[0])
    }

    /// The defect itself: B's only window is gone, and A still connects.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_account_with_every_window_closed_accepts_a_connect() -> Result<(), Box<dyn Error>> {
        setup_log();
        let mut agents = two_registered_agents().await?;
        let (cid_a, cid_b) = (agents[0].2, agents[1].2);
        drop(agents.pop()); // B's window closes; the session stays signed in.

        let (tx_a, rx_a, _) = &mut agents[0];
        tx_a.send(peer_connect(cid_a, cid_b, SecurityLevel::Standard))?;
        outcome(rx_a, cid_b).await?;
        Ok(())
    }

    /// B's window is open but shows something else, so it never answers. It is
    /// told the agent has the offer, which is what keeps a window from answering too.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_window_is_told_the_agent_answers() -> Result<(), Box<dyn Error>> {
        setup_log();
        let mut agents = two_registered_agents().await?;
        let ((tx_a, rx_a, cid_a), b) = split(&mut agents);
        tx_a.send(peer_connect(*cid_a, b.2, SecurityLevel::Standard))?;
        assert!(answered_by_agent(&mut b.1).await);
        outcome(rx_a, b.2).await?;
        Ok(())
    }

    /// A paused contact is declined: the initiator is told at once, not left to time out.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_paused_contact_is_declined() -> Result<(), Box<dyn Error>> {
        setup_log();
        let mut agents = two_registered_agents().await?;
        let ((tx_a, rx_a, cid_a), b) = split(&mut agents);
        b.0.send(pause(b.2, *cid_a))?;
        next_matching(&mut b.1, "LocalDBSetKVSuccess", |r| {
            matches!(r, InternalServiceResponse::LocalDBSetKVSuccess(_)).then_some(())
        })
        .await;
        tx_a.send(peer_connect(*cid_a, b.2, SecurityLevel::Standard))?;
        assert!(
            outcome(rx_a, b.2).await.is_err(),
            "a paused contact connected"
        );
        Ok(())
    }

    /// An offer below the chat's minimum is declined; one at it is accepted.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_offer_below_the_chats_level_is_declined() -> Result<(), Box<dyn Error>> {
        setup_log();
        let mut agents = two_registered_agents().await?;
        let ((tx_a, rx_a, cid_a), b) = split(&mut agents);
        b.0.send(require_level(b.2, *cid_a, ChatSecurityLevel::Reinforced))?;
        next_matching(&mut b.1, "AccountPreferencesResponse", |r| {
            matches!(r, InternalServiceResponse::AccountPreferencesResponse(_)).then_some(())
        })
        .await;

        tx_a.send(peer_connect(*cid_a, b.2, SecurityLevel::Standard))?;
        assert!(
            outcome(rx_a, b.2).await.is_err(),
            "a Standard offer was admitted"
        );
        tx_a.send(peer_connect(*cid_a, b.2, SecurityLevel::Reinforced))?;
        outcome(rx_a, b.2).await?;
        Ok(())
    }

    /// Both hosted, both dial at once: each agent answers the other's offer, and
    /// the pair connects (the SDK settles the race; one success is enough).
    #[tokio::test(flavor = "multi_thread")]
    async fn simultaneous_connects_between_hosted_accounts_connect() -> Result<(), Box<dyn Error>> {
        setup_log();
        let mut agents = two_registered_agents().await?;
        let (a, b) = split(&mut agents);
        a.0.send(peer_connect(a.2, b.2, SecurityLevel::Standard))?;
        b.0.send(peer_connect(b.2, a.2, SecurityLevel::Standard))?;
        let (at_a, at_b) = tokio::join!(outcome(&mut a.1, b.2), outcome(&mut b.1, a.2));
        assert!(at_a.is_ok() || at_b.is_ok(), "{at_a:?} / {at_b:?}");
        Ok(())
    }
}

//! Two windows open the same chat at different levels, and one side's agent answers the peer's
//! offer before its own window's dial arrives. That ordering is the one a loaded machine
//! produces for `peer_security_level::simultaneous_offers_at_different_levels_still_connect`;
//! here it is forced, by waiting for the agent's answer before the second dial is sent.
//!
//! `cargo test -p citadel-internal-service --test a_dial_after_an_accept_still_connects -- --include-ignored`

use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::setup_log;
    use crate::common::turn_harness::{next_matching, two_registered_agents, STEP};
    use crate::common::PeerReturnHandle;
    use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
    use citadel_sdk::prelude::*;
    use std::error::Error;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
    use uuid::Uuid;

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

    /// Waits until `rx`'s agent has answered `from`'s offer itself.
    async fn answered_by_agent(rx: &mut UnboundedReceiver<InternalServiceResponse>, from: u64) {
        let answered = next_matching(rx, "PeerConnectNotification", |r| match r {
            InternalServiceResponse::PeerConnectNotification(n) if n.peer_cid == from => {
                Some(n.answered_by_agent)
            }
            _ => None,
        })
        .await;
        assert!(answered, "the agent left the offer to windows");
    }

    async fn connect_outcome(
        rx: &mut UnboundedReceiver<InternalServiceResponse>,
        peer: u64,
    ) -> Result<(), String> {
        next_matching(rx, "PeerConnectSuccess/Failure", |r| match r {
            InternalServiceResponse::PeerConnectSuccess(s) if s.peer_cid == peer => Some(Ok(())),
            InternalServiceResponse::PeerConnectFailure(f) => Some(Err(f.message)),
            _ => None,
        })
        .await
    }

    async fn crosses(
        tx: &UnboundedSender<InternalServiceRequest>,
        rx: &mut UnboundedReceiver<InternalServiceResponse>,
        rx_peer: &mut UnboundedReceiver<InternalServiceResponse>,
        (cid, peer_cid): (u64, u64),
        body: &[u8],
    ) -> bool {
        tx.send(InternalServiceRequest::Message {
            request_id: Uuid::new_v4(),
            message: body.to_vec(),
            cid,
            peer_cid: Some(peer_cid),
            security_level: SecurityLevel::Standard,
        })
        .unwrap();
        let sent = next_matching(rx, "MessageSendSuccess/Failure", |r| match r {
            InternalServiceResponse::MessageSendSuccess(_) => Some(true),
            InternalServiceResponse::MessageSendFailure(_) => Some(false),
            _ => None,
        })
        .await;
        if !sent {
            return false;
        }
        let deadline = tokio::time::Instant::now() + STEP;
        loop {
            match tokio::time::timeout_at(deadline, rx_peer.recv()).await {
                Ok(Some(InternalServiceResponse::MessageNotification(n)))
                    if n.peer_cid == cid && &*n.message == body =>
                {
                    return true
                }
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => return false,
            }
        }
    }

    fn split(agents: &mut [PeerReturnHandle]) -> (&mut PeerReturnHandle, &mut PeerReturnHandle) {
        let (a, b) = agents.split_at_mut(1);
        (&mut a[0], &mut b[0])
    }

    /// B offers Standard; A's agent accepts it; only then does A's window dial at High. The
    /// pair connects, and Standard traffic crosses both ways.
    #[ignore = "at Citadel-Protocol 1a6d4262 A refuses the exchange its own accept started, judged against its later High dial, and neither side connects; un-ignore with the SDK pin that keeps the displaced accept"]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_stronger_dial_after_accepting_a_weaker_offer_still_connects(
    ) -> Result<(), Box<dyn Error>> {
        setup_log();
        let mut agents = two_registered_agents().await?;
        let ((tx_a, rx_a, cid_a), (tx_b, rx_b, cid_b)) = split(&mut agents);
        let (cid_a, cid_b) = (*cid_a, *cid_b);

        tx_b.send(peer_connect(cid_b, cid_a, SecurityLevel::Standard))
            .unwrap();
        answered_by_agent(rx_a, cid_b).await;
        tx_a.send(peer_connect(cid_a, cid_b, SecurityLevel::High))
            .unwrap();
        let (a, b) = tokio::join!(connect_outcome(rx_a, cid_b), connect_outcome(rx_b, cid_a));
        println!("[a_dial_after_an_accept] a {a:?}, b {b:?}");
        assert!(
            a.is_ok() || b.is_ok(),
            "neither side connected: {a:?} / {b:?}"
        );
        assert!(crosses(tx_a, rx_a, rx_b, (cid_a, cid_b), b"a").await);
        assert!(crosses(tx_b, rx_b, rx_a, (cid_b, cid_a), b"b").await);
        Ok(())
    }
}

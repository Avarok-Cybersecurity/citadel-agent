//! Per-chat encryption levels, as the UI now uses them.
//!
//! The UI keeps a chat's level three ways: it opens the peer connection at that level, it
//! declines a PeerConnect offered below it (and opens the channel itself), and it sends every
//! message at it. These pin what the agent and SDK do with each, with no live services: two
//! agents on one in-process server.
//!
//! `cargo test -p citadel-internal-service --test peer_security_level -- --include-ignored`
//!
//! Three are ignored: at Citadel-Protocol a49af50 (integ/live-sdk) they fail, and they are
//! why the UI's per-chat encryption level stays disabled. Found 2026-09-25:
//!   - A PeerConnect at a level above the session's login level makes the SDK craft a peer
//!     signal at that level over the C2S ratchet; it fails verify_level ("Only have max 0
//!     security levels") and the whole C2S session ends.
//!   - A message asking for High over a Standard P2P channel is sent and delivered: the
//!     per-message level is not bounded by the channel's ratchet depth on this path.
//!
//! Un-ignore them to verify an SDK fix.

use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::setup_log;
    use crate::common::turn_harness::{next_matching, two_registered_agents, STEP};
    use crate::common::PeerReturnHandle;
    use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
    use citadel_sdk::prelude::*;
    use std::error::Error;
    use std::time::Duration;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
    use uuid::Uuid;

    fn at(level: SecurityLevel) -> SessionSecuritySettings {
        SessionSecuritySettingsBuilder::default()
            .with_security_level(level)
            .build()
            .unwrap()
    }

    fn peer_connect(cid: u64, peer_cid: u64, level: SecurityLevel) -> InternalServiceRequest {
        InternalServiceRequest::PeerConnect {
            request_id: Uuid::new_v4(),
            cid,
            peer_cid,
            udp_mode: UdpMode::Disabled,
            session_security_settings: at(level),
            peer_session_password: None,
            turn: None,
        }
    }

    /// The UI's answer: the acceptor's settings are sent as Standard to show they are ignored.
    fn answer(cid: u64, peer_cid: u64, accept: bool) -> InternalServiceRequest {
        InternalServiceRequest::PeerConnectAccept {
            request_id: Uuid::new_v4(),
            cid,
            peer_cid,
            accept,
            udp_mode: UdpMode::Disabled,
            session_security_settings: at(SecurityLevel::Standard),
            peer_session_password: None,
            turn: None,
        }
    }

    /// The level the initiator's offer names, as the acceptor's UI sees it.
    async fn offered_level(
        rx: &mut UnboundedReceiver<InternalServiceResponse>,
        from: u64,
    ) -> SecurityLevel {
        next_matching(rx, "PeerConnectNotification", |r| match r {
            InternalServiceResponse::PeerConnectNotification(n) if n.peer_cid == from => {
                Some(n.session_security_settings.security_level)
            }
            _ => None,
        })
        .await
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

    /// Sends one message at `level`; answers the agent's verdict on the SEND.
    async fn send_at(
        tx: &UnboundedSender<InternalServiceRequest>,
        rx: &mut UnboundedReceiver<InternalServiceResponse>,
        cid: u64,
        peer_cid: u64,
        level: SecurityLevel,
        body: &[u8],
    ) -> Result<(), String> {
        tx.send(InternalServiceRequest::Message {
            request_id: Uuid::new_v4(),
            message: body.to_vec(),
            cid,
            peer_cid: Some(peer_cid),
            security_level: level,
        })
        .unwrap();
        next_matching(rx, "MessageSendSuccess/Failure", |r| match r {
            InternalServiceResponse::MessageSendSuccess(_) => Some(Ok(())),
            InternalServiceResponse::MessageSendFailure(f) => Some(Err(f.message)),
            _ => None,
        })
        .await
    }

    /// Whether `body` from `from` reaches `rx` within `wait`.
    async fn arrives(
        rx: &mut UnboundedReceiver<InternalServiceResponse>,
        from: u64,
        body: &[u8],
        wait: Duration,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(InternalServiceResponse::MessageNotification(n)))
                    if n.peer_cid == from && &*n.message == body =>
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

    /// A opens at High, B accepts; both send at High and both messages arrive.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_channel_opened_at_high_carries_high_messages_both_ways() -> Result<(), Box<dyn Error>>
    {
        setup_log();
        let mut agents = two_registered_agents().await?;
        let ((tx_a, rx_a, cid_a), (tx_b, rx_b, cid_b)) = split(&mut agents);
        let (cid_a, cid_b) = (*cid_a, *cid_b);

        tx_a.send(peer_connect(cid_a, cid_b, SecurityLevel::High))
            .unwrap();
        assert_eq!(
            offered_level(rx_b, cid_a).await.value(),
            SecurityLevel::High.value()
        );
        tx_b.send(answer(cid_b, cid_a, true)).unwrap();
        connect_outcome(rx_a, cid_b).await?;

        send_at(tx_a, rx_a, cid_a, cid_b, SecurityLevel::High, b"a at high").await?;
        assert!(arrives(rx_b, cid_a, b"a at high", STEP).await);
        send_at(tx_b, rx_b, cid_b, cid_a, SecurityLevel::High, b"b at high").await?;
        assert!(arrives(rx_a, cid_b, b"b at high", STEP).await);
        Ok(())
    }

    /// Over a Standard channel, a message asking for High is not delivered at a lower level,
    /// and the agent is still serving afterwards: a Standard message still crosses.
    #[ignore = "a High message over a Standard channel is still sent and delivered (the per-message level is not bounded by the channel); the UI opens the channel at the chat level, so it never asks for more"]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_message_above_the_channel_level_is_not_downgraded() -> Result<(), Box<dyn Error>> {
        setup_log();
        let mut agents = two_registered_agents().await?;
        let ((tx_a, rx_a, cid_a), (tx_b, rx_b, cid_b)) = split(&mut agents);
        let (cid_a, cid_b) = (*cid_a, *cid_b);

        tx_a.send(peer_connect(cid_a, cid_b, SecurityLevel::Standard))
            .unwrap();
        offered_level(rx_b, cid_a).await;
        tx_b.send(answer(cid_b, cid_a, true)).unwrap();
        connect_outcome(rx_a, cid_b).await?;

        let verdict = send_at(tx_a, rx_a, cid_a, cid_b, SecurityLevel::High, b"too high").await;
        let delivered = arrives(rx_b, cid_a, b"too high", Duration::from_secs(5)).await;
        println!("[peer_security_level] High over Standard: send verdict {verdict:?}, delivered {delivered}");
        assert!(!delivered, "a High message crossed a Standard channel");

        send_at(
            tx_a,
            rx_a,
            cid_a,
            cid_b,
            SecurityLevel::Standard,
            b"standard",
        )
        .await?;
        assert!(arrives(rx_b, cid_a, b"standard", STEP).await);
        Ok(())
    }

    /// The UI's convergence: B requires High and declines A's Standard offer; A's connect fails
    /// (it is answered, not left hanging); B then opens at High and A accepts. Both then send
    /// at their own levels and both arrive.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_declined_low_offer_is_replaced_by_the_higher_one() -> Result<(), Box<dyn Error>> {
        setup_log();
        let mut agents = two_registered_agents().await?;
        let ((tx_a, rx_a, cid_a), (tx_b, rx_b, cid_b)) = split(&mut agents);
        let (cid_a, cid_b) = (*cid_a, *cid_b);

        tx_a.send(peer_connect(cid_a, cid_b, SecurityLevel::Standard))
            .unwrap();
        assert_eq!(
            offered_level(rx_b, cid_a).await.value(),
            SecurityLevel::Standard.value()
        );
        tx_b.send(answer(cid_b, cid_a, false)).unwrap();
        let refused = tokio::time::timeout(Duration::from_secs(45), connect_outcome(rx_a, cid_b))
            .await
            .expect("a declined PeerConnect was never answered");
        assert!(refused.is_err(), "a declined offer reported success");

        tx_b.send(peer_connect(cid_b, cid_a, SecurityLevel::High))
            .unwrap();
        assert_eq!(
            offered_level(rx_a, cid_b).await.value(),
            SecurityLevel::High.value()
        );
        tx_a.send(answer(cid_a, cid_b, true)).unwrap();
        connect_outcome(rx_b, cid_a).await?;

        send_at(tx_b, rx_b, cid_b, cid_a, SecurityLevel::High, b"b at high").await?;
        assert!(arrives(rx_a, cid_b, b"b at high", STEP).await);
        send_at(
            tx_a,
            rx_a,
            cid_a,
            cid_b,
            SecurityLevel::Standard,
            b"a at standard",
        )
        .await?;
        assert!(arrives(rx_b, cid_a, b"a at standard", STEP).await);
        Ok(())
    }

    /// Two browsers both open the channel, at different levels (each side's auto-connect
    /// initiates). The SDK settles the race; whatever it settles on, the pair connects and
    /// Standard traffic crosses both ways -- no crash, no hang.
    #[tokio::test(flavor = "multi_thread")]
    async fn simultaneous_offers_at_different_levels_still_connect() -> Result<(), Box<dyn Error>> {
        setup_log();
        let mut agents = two_registered_agents().await?;
        let ((tx_a, rx_a, cid_a), (tx_b, rx_b, cid_b)) = split(&mut agents);
        let (cid_a, cid_b) = (*cid_a, *cid_b);

        tx_a.send(peer_connect(cid_a, cid_b, SecurityLevel::High))
            .unwrap();
        tx_b.send(peer_connect(cid_b, cid_a, SecurityLevel::Standard))
            .unwrap();
        let (a, b) = tokio::join!(connect_outcome(rx_a, cid_b), connect_outcome(rx_b, cid_a));
        println!("[peer_security_level] simultaneous: a {a:?}, b {b:?}");
        assert!(
            a.is_ok() || b.is_ok(),
            "neither side connected: {a:?} / {b:?}"
        );

        send_at(tx_a, rx_a, cid_a, cid_b, SecurityLevel::Standard, b"a").await?;
        assert!(arrives(rx_b, cid_a, b"a", STEP).await);
        send_at(tx_b, rx_b, cid_b, cid_a, SecurityLevel::Standard, b"b").await?;
        assert!(arrives(rx_a, cid_b, b"b", STEP).await);
        Ok(())
    }
}

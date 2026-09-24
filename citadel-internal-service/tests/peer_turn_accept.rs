//! TURN over the browser's connect shape: the initiator sends PeerConnect, the other side
//! answers with PeerConnectAccept. Each carries its own half of the relay config; the acceptor's
//! success comes from its channel being created and must report the same path.
//!
//! Needs `turnserver` on PATH; ignored by default:
//! `cargo nextest run -p citadel-internal-service --features websockets --test peer_turn_accept
//! --run-ignored all`.

use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::coturn::{Coturn, PASSWORD, USER};
    use crate::common::setup_log;
    use crate::common::turn_harness::*;
    use citadel_internal_service_types::{P2pPathReport, PeerTurnConfig, TurnPolicy};
    use std::error::Error;

    fn coturn_config(coturn: &Coturn, policy: TurnPolicy) -> PeerTurnConfig {
        config(
            policy,
            vec![coturn.url("udp")],
            USER,
            PASSWORD,
            unix_now() + 300,
        )
    }

    #[ignore = "needs coturn (turnserver) on PATH"]
    #[tokio::test(flavor = "multi_thread")]
    async fn relay_only_on_both_halves_of_an_accept_reports_turn() -> Result<(), Box<dyn Error>> {
        setup_log();
        let coturn = Coturn::start();
        let turn = coturn_config(&coturn, TurnPolicy::RelayOnly);
        let mut agents = two_registered_agents().await?;
        let outcome = connect_by_accept(&mut agents, Some(turn.clone()), Some(turn)).await;
        assert_eq!(outcome.initiator, Ok(P2pPathReport::Turn), "{outcome:?}");
        assert!(outcome.accept_delivered, "{outcome:?}");
        assert_eq!(outcome.acceptor, Some(P2pPathReport::Turn), "{outcome:?}");
        message_each_way(&mut agents).await;
        media_datagram_crosses(&mut agents).await;
        Ok(())
    }

    /// Control: the acceptor sends no config. With `fallback` the initiator can still hole
    /// punch — on loopback that succeeds — so no relay is used.
    #[ignore = "needs coturn (turnserver) on PATH"]
    #[tokio::test(flavor = "multi_thread")]
    async fn fallback_with_no_acceptor_config_connects_directly() -> Result<(), Box<dyn Error>> {
        setup_log();
        let coturn = Coturn::start();
        let turn = coturn_config(&coturn, TurnPolicy::Fallback);
        let mut agents = two_registered_agents().await?;
        let outcome = connect_by_accept(&mut agents, Some(turn), None).await;
        assert_eq!(outcome.initiator, Ok(P2pPathReport::Direct), "{outcome:?}");
        assert_eq!(outcome.acceptor, Some(P2pPathReport::Direct), "{outcome:?}");
        message_each_way(&mut agents).await;
        Ok(())
    }

    /// Control: relay-only on the initiator, nothing on the acceptor. The initiator skips the
    /// hole punch and has no relay partner; the acceptor's hole punch has no counterpart and
    /// gives up after the SDK's 30s timeout. Both end server-relayed, and messages still flow.
    #[ignore = "needs coturn (turnserver) on PATH"]
    #[tokio::test(flavor = "multi_thread")]
    async fn relay_only_with_no_acceptor_config_stays_server_relayed() -> Result<(), Box<dyn Error>>
    {
        setup_log();
        let coturn = Coturn::start();
        let turn = coturn_config(&coturn, TurnPolicy::RelayOnly);
        let mut agents = two_registered_agents().await?;
        let outcome = connect_by_accept(&mut agents, Some(turn), None).await;
        assert_eq!(
            outcome.initiator,
            Ok(P2pPathReport::ServerRelay),
            "{outcome:?}"
        );
        assert_eq!(
            outcome.acceptor,
            Some(P2pPathReport::ServerRelay),
            "{outcome:?}"
        );
        message_each_way(&mut agents).await;
        Ok(())
    }
}

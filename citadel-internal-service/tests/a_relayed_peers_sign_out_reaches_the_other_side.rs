//! A peer that signs out is gone for the other side, whichever path their connection took.
//!
//! Seen on CI (citadel-workspace run 38169743447, test:reconnect-p2p-one-c2s): user1 signed
//! out and back in; user2's agent kept its sink to user1's old connection and ILM sent into
//! it every 200 ms ("stream died") while user1's agent had no peer to send on. Nothing ever
//! told user2's agent the connection was over, so nothing redialled it.
//!
//! When a session ends, the server tells each of its peers with a `PeerSignal::Disconnect`
//! that names no connection (Citadel-Protocol `session_manager.rs`, `disconnect_token: None`),
//! and the receiving SDK drops its virtual connection to that peer. Only a DIRECT connection
//! has a drop notifier, so only a direct connection is also reported named. A connection
//! still relayed through the server is reported by the unnamed notice alone, and the agent
//! kept any named connection on an unnamed report.
use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::setup_log;
    use crate::common::turn_harness::*;
    use citadel_internal_service_types::{
        InternalServiceRequest, InternalServiceResponse, P2pPathReport, TurnPolicy,
    };
    use std::error::Error;
    use uuid::Uuid;

    /// Never hole-punched: the expired relay-only config keeps the pair on the server relay
    /// (peer_turn.rs 4b), so no direct connection exists to report its own end.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_server_relayed_peers_sign_out_ends_the_connection_for_the_other(
    ) -> Result<(), Box<dyn Error>> {
        setup_log();
        let relay_only = config(
            TurnPolicy::RelayOnly,
            vec![dead_turn_url()],
            "u",
            "p",
            unix_now() - 1,
        );
        sign_out_reaches_the_other_side(
            [Some(relay_only.clone()), Some(relay_only)],
            P2pPathReport::ServerRelay,
        )
        .await
    }

    /// Control: the direct connection's own end is reported named, and always was.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_direct_peers_sign_out_ends_the_connection_for_the_other(
    ) -> Result<(), Box<dyn Error>> {
        setup_log();
        sign_out_reaches_the_other_side([None, None], P2pPathReport::Direct).await
    }

    async fn sign_out_reaches_the_other_side(
        turn: [Option<citadel_internal_service_types::PeerTurnConfig>; 2],
        path: P2pPathReport,
    ) -> Result<(), Box<dyn Error>> {
        let mut agents = two_registered_agents().await?;
        assert_eq!(connect(&mut agents, turn).await?, [path, path]);
        message_each_way(&mut agents).await;

        let (a, b) = agents.split_at_mut(1);
        let ((tx_a, _, cid_a), (_, rx_b, cid_b)) = (&mut a[0], &mut b[0]);
        let (cid_a, cid_b) = (*cid_a, *cid_b);
        tx_a.send(InternalServiceRequest::Disconnect {
            request_id: Uuid::new_v4(),
            cid: cid_a,
        })?;

        next_matching(rx_b, "B told that A's connection ended", |r| match r {
            InternalServiceResponse::DisconnectNotification(n)
                if n.cid == cid_b && n.peer_cid == Some(cid_a) =>
            {
                Some(())
            }
            _ => None,
        })
        .await;
        Ok(())
    }
}

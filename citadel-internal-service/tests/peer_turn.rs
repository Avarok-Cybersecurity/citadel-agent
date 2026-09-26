//! PeerConnect's `turn` field: two agents reach each other through a TURN relay when both send
//! one, report the settled path in `PeerConnectSuccess.path`, and carry messages and media over
//! it. Controls: no config, an expired config, and a fallback config on loopback all connect
//! directly.
//!
//! The coturn case needs `turnserver` on PATH, and the live case freshly minted Cloudflare
//! credentials; both are ignored by default:
//! `cargo nextest run -p citadel-internal-service --features websockets --test peer_turn
//! --run-ignored all`.

use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::coturn::{Coturn, PASSWORD, USER};
    use crate::common::setup_log;
    use crate::common::turn_harness::*;
    use citadel_internal_service_types::{IceServer, P2pPathReport, PeerTurnConfig, TurnPolicy};
    use std::error::Error;

    /// 1. Relay-only through a local coturn on both sides: both report `turn`, and messages and
    ///    a media datagram cross the relay.
    #[ignore = "needs coturn (turnserver) on PATH"]
    #[tokio::test(flavor = "multi_thread")]
    async fn relay_only_with_coturn_reports_turn_and_carries_messages_and_media(
    ) -> Result<(), Box<dyn Error>> {
        setup_log();
        let coturn = Coturn::start();
        let turn = config(
            TurnPolicy::RelayOnly,
            vec![coturn.url("udp")],
            USER,
            PASSWORD,
            unix_now() + 300,
        );
        let mut agents = two_registered_agents().await?;
        let paths = connect(&mut agents, [Some(turn.clone()), Some(turn)]).await?;
        assert_eq!(paths, [P2pPathReport::Turn, P2pPathReport::Turn]);
        message_each_way(&mut agents).await;
        media_datagram_crosses(&mut agents).await;
        Ok(())
    }

    /// Control for 1: the same coturn with a wrong password refuses the allocation, so the pair
    /// stays server-relayed — the `turn` above was coturn authenticating the agents'
    /// credentials, not a path reported regardless.
    #[ignore = "needs coturn (turnserver) on PATH"]
    #[tokio::test(flavor = "multi_thread")]
    async fn relay_only_with_a_wrong_coturn_password_stays_server_relayed(
    ) -> Result<(), Box<dyn Error>> {
        setup_log();
        let coturn = Coturn::start();
        let turn = config(
            TurnPolicy::RelayOnly,
            vec![coturn.url("udp")],
            USER,
            "not-the-password",
            unix_now() + 300,
        );
        let mut agents = two_registered_agents().await?;
        let paths = connect(&mut agents, [Some(turn.clone()), Some(turn)]).await?;
        assert_eq!(
            paths,
            [P2pPathReport::ServerRelay, P2pPathReport::ServerRelay],
            "{}",
            coturn.log()
        );
        message_each_way(&mut agents).await;
        Ok(())
    }

    /// 2. Fallback on loopback: the direct path is tried first and succeeds, so no relay.
    #[tokio::test(flavor = "multi_thread")]
    async fn fallback_on_loopback_connects_directly() -> Result<(), Box<dyn Error>> {
        setup_log();
        let turn = config(
            TurnPolicy::Fallback,
            vec![dead_turn_url()],
            "u",
            "p",
            unix_now() + 300,
        );
        let mut agents = two_registered_agents().await?;
        let paths = connect(&mut agents, [Some(turn.clone()), Some(turn)]).await?;
        assert_eq!(paths, [P2pPathReport::Direct, P2pPathReport::Direct]);
        message_each_way(&mut agents).await;
        Ok(())
    }

    /// 3. No turn config: unchanged — direct on loopback.
    #[tokio::test(flavor = "multi_thread")]
    async fn without_a_turn_config_the_pair_connects_directly() -> Result<(), Box<dyn Error>> {
        setup_log();
        let mut agents = two_registered_agents().await?;
        let paths = connect(&mut agents, [None, None]).await?;
        assert_eq!(paths, [P2pPathReport::Direct, P2pPathReport::Direct]);
        message_each_way(&mut agents).await;
        media_datagram_crosses(&mut agents).await;
        Ok(())
    }

    /// 4. An expired Fallback config is no config: the pair connects directly.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_expired_fallback_config_is_treated_as_none() -> Result<(), Box<dyn Error>> {
        setup_log();
        let expired = config(TurnPolicy::Fallback, vec![dead_turn_url()], "u", "p", unix_now() - 1);
        let mut agents = two_registered_agents().await?;
        let paths = connect(&mut agents, [Some(expired.clone()), Some(expired)]).await?;
        assert_eq!(paths, [P2pPathReport::Direct, P2pPathReport::Direct]);
        Ok(())
    }

    /// 4b. An expired RELAY-ONLY config never connects directly: relay-only promises the peers
    ///     never learn each other's addresses, so with no usable relay the pair stays relayed
    ///     through the server. (It used to be treated as no config -- a direct connection.)
    #[tokio::test(flavor = "multi_thread")]
    async fn an_expired_relay_only_config_stays_server_relayed() -> Result<(), Box<dyn Error>> {
        setup_log();
        let expired = config(TurnPolicy::RelayOnly, vec![dead_turn_url()], "u", "p", unix_now() - 1);
        let mut agents = two_registered_agents().await?;
        let paths = connect(&mut agents, [Some(expired.clone()), Some(expired)]).await?;
        assert_eq!(paths, [P2pPathReport::ServerRelay, P2pPathReport::ServerRelay]);
        message_each_way(&mut agents).await;
        Ok(())
    }

    /// 4c. Relay-only on ONE side (the other sends none) should still never connect directly,
    ///     and should connect. Today it does neither: the other side's direct attempt waits for
    ///     a hole punch that never comes and PeerConnect times out after 30 s. This is why the
    ///     UI offers no per-chat relay choice: a policy one person sets must reach the other
    ///     side (it is not carried in the offer), or the chat breaks.
    #[ignore = "one-sided relay-only times out: the policy is not carried in the offer"]
    #[tokio::test(flavor = "multi_thread")]
    async fn relay_only_on_one_side_never_connects_directly() -> Result<(), Box<dyn Error>> {
        setup_log();
        let expired = config(TurnPolicy::RelayOnly, vec![dead_turn_url()], "u", "p", unix_now() - 1);
        let mut agents = two_registered_agents().await?;
        let paths = connect(&mut agents, [Some(expired), None]).await?;
        assert!(!paths.contains(&P2pPathReport::Direct), "a relay-only side connected directly: {paths:?}");
        message_each_way(&mut agents).await;
        Ok(())
    }

    /// Control for 4: the same config, unexpired, IS used — relay-only through a dead relay
    /// leaves the pair server-relayed, so test 4 discriminates.
    #[tokio::test(flavor = "multi_thread")]
    async fn relay_only_through_a_dead_relay_stays_server_relayed() -> Result<(), Box<dyn Error>> {
        setup_log();
        let live = config(
            TurnPolicy::RelayOnly,
            vec![dead_turn_url()],
            "u",
            "p",
            unix_now() + 300,
        );
        let mut agents = two_registered_agents().await?;
        let paths = connect(&mut agents, [Some(live.clone()), Some(live)]).await?;
        assert_eq!(
            paths,
            [P2pPathReport::ServerRelay, P2pPathReport::ServerRelay]
        );
        message_each_way(&mut agents).await;
        Ok(())
    }

    /// Live smoke (manual): relay-only through Cloudflare Realtime TURN with a freshly minted
    /// short-TTL credential in `CF_TURN_USERNAME` / `CF_TURN_CREDENTIAL`, in the shape
    /// `GetIceServers` returns.
    #[ignore = "live: needs CF_TURN_USERNAME / CF_TURN_CREDENTIAL minted from a Cloudflare TURN key"]
    #[tokio::test(flavor = "multi_thread")]
    async fn relay_only_through_cloudflare_reports_turn() -> Result<(), Box<dyn Error>> {
        setup_log();
        let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} must be set"));
        let turn = PeerTurnConfig {
            policy: TurnPolicy::RelayOnly,
            ice_servers: vec![
                IceServer {
                    urls: vec!["stun:stun.cloudflare.com:3478".into()],
                    username: None,
                    credential: None,
                },
                IceServer {
                    urls: vec![
                        "turn:turn.cloudflare.com:3478?transport=udp".into(),
                        "turn:turn.cloudflare.com:80?transport=tcp".into(),
                        "turns:turn.cloudflare.com:443?transport=tcp".into(),
                    ],
                    username: Some(var("CF_TURN_USERNAME")),
                    credential: Some(var("CF_TURN_CREDENTIAL")),
                },
            ],
            expires_at: unix_now() + 300,
        };
        let mut agents = two_registered_agents().await?;
        let paths = connect(&mut agents, [Some(turn.clone()), Some(turn)]).await?;
        assert_eq!(paths, [P2pPathReport::Turn, P2pPathReport::Turn]);
        message_each_way(&mut agents).await;
        Ok(())
    }
}

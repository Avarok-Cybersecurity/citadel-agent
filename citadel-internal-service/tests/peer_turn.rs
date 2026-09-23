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
    use crate::common::{
        connect_p2p_with_turn, get_free_port, register_p2p, services_connected_to_one_server,
        setup_log, PeerReturnHandle,
    };
    use citadel_internal_service_types::{
        IceServer, InternalServiceRequest, InternalServiceResponse, P2pPathReport, PeerTurnConfig,
        TurnPolicy,
    };
    use citadel_sdk::prelude::*;
    use std::error::Error;
    use std::net::SocketAddr;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
    use uuid::Uuid;

    const STEP: Duration = Duration::from_secs(30);

    fn unix_now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn config(
        policy: TurnPolicy,
        urls: Vec<String>,
        user: &str,
        pass: &str,
        expires_at: u64,
    ) -> PeerTurnConfig {
        PeerTurnConfig {
            policy,
            ice_servers: vec![IceServer {
                urls,
                username: Some(user.to_string()),
                credential: Some(pass.to_string()),
            }],
            expires_at,
        }
    }

    /// A TURN URL nothing listens on: relaying through it can never succeed.
    fn dead_turn_url() -> String {
        let dead = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        format!("turn:{}?transport=udp", dead.local_addr().unwrap())
    }

    async fn two_registered_agents() -> Result<Vec<PeerReturnHandle>, Box<dyn Error>> {
        let addrs: Vec<SocketAddr> = (0..2)
            .map(|_| format!("127.0.0.1:{}", get_free_port()).parse().unwrap())
            .collect();
        let mut agents = services_connected_to_one_server::<StackedRatchet>(addrs, None).await?;
        let (a, b) = agents.split_at_mut(1);
        let ((tx_a, rx_a, cid_a), (tx_b, rx_b, cid_b)) = (&mut a[0], &mut b[0]);
        register_p2p(
            tx_a,
            rx_a,
            *cid_a,
            tx_b,
            rx_b,
            *cid_b,
            SessionSecuritySettings::default(),
            None,
        )
        .await?;
        Ok(agents)
    }

    /// Brings the pair up with `turn` on both sides; returns both reported paths.
    async fn connect(
        agents: &mut [PeerReturnHandle],
        turn: [Option<PeerTurnConfig>; 2],
    ) -> Result<[P2pPathReport; 2], Box<dyn Error>> {
        let (a, b) = agents.split_at_mut(1);
        let ((tx_a, rx_a, cid_a), (tx_b, rx_b, cid_b)) = (&mut a[0], &mut b[0]);
        tokio::time::timeout(
            Duration::from_secs(60),
            connect_p2p_with_turn(
                tx_a,
                rx_a,
                *cid_a,
                tx_b,
                rx_b,
                *cid_b,
                SessionSecuritySettings::default(),
                None,
                UdpMode::Enabled,
                turn,
            ),
        )
        .await
        .expect("PeerConnect did not settle within 60s")
    }

    async fn next_matching<T>(
        rx: &mut UnboundedReceiver<InternalServiceResponse>,
        what: &str,
        mut pick: impl FnMut(InternalServiceResponse) -> Option<T>,
    ) -> T {
        let deadline = tokio::time::Instant::now() + STEP;
        loop {
            let got = tokio::time::timeout_at(deadline, rx.recv())
                .await
                .unwrap_or_else(|_| panic!("no {what} within {STEP:?}"))
                .expect("service channel closed");
            if let Some(v) = pick(got) {
                return v;
            }
        }
    }

    async fn message_one_way(
        tx: &UnboundedSender<InternalServiceRequest>,
        cid: u64,
        rx_peer: &mut UnboundedReceiver<InternalServiceResponse>,
        peer_cid: u64,
    ) {
        let body = format!("hello from {cid}").into_bytes();
        tx.send(InternalServiceRequest::Message {
            request_id: Uuid::new_v4(),
            message: body.clone(),
            cid,
            peer_cid: Some(peer_cid),
            security_level: Default::default(),
        })
        .unwrap();
        let got = next_matching(rx_peer, "MessageNotification", |r| match r {
            InternalServiceResponse::MessageNotification(n) if n.peer_cid == cid => Some(n),
            _ => None,
        })
        .await;
        assert_eq!(got.cid, peer_cid);
        assert_eq!(got.message, body);
    }

    async fn message_each_way(agents: &mut [PeerReturnHandle]) {
        let (a, b) = agents.split_at_mut(1);
        let ((tx_a, rx_a, cid_a), (tx_b, rx_b, cid_b)) = (&mut a[0], &mut b[0]);
        message_one_way(tx_a, *cid_a, rx_b, *cid_b).await;
        message_one_way(tx_b, *cid_b, rx_a, *cid_a).await;
    }

    async fn open_media(
        tx: &UnboundedSender<InternalServiceRequest>,
        rx: &mut UnboundedReceiver<InternalServiceResponse>,
        cid: u64,
        peer_cid: u64,
    ) {
        tx.send(InternalServiceRequest::MediaOpen {
            request_id: Uuid::new_v4(),
            cid,
            peer_cid,
        })
        .unwrap();
        let opened = next_matching(rx, "media session result", |r| match r {
            InternalServiceResponse::MediaSessionOpened(o) => Some(o),
            InternalServiceResponse::MediaSessionFailed(f) => {
                panic!("media open failed for {cid}: {}", f.message)
            }
            _ => None,
        })
        .await;
        assert!(opened.unreliable, "{cid}: media has no datagram path");
    }

    /// Both sides open a media session; A's frames reach B. Frames are datagrams and may be
    /// dropped, so A resends until one arrives.
    async fn media_datagram_crosses(agents: &mut [PeerReturnHandle]) {
        let (a, b) = agents.split_at_mut(1);
        let ((tx_a, rx_a, cid_a), (tx_b, rx_b, cid_b)) = (&mut a[0], &mut b[0]);
        open_media(tx_a, rx_a, *cid_a, *cid_b).await;
        open_media(tx_b, rx_b, *cid_b, *cid_a).await;
        let payload = vec![0xC7u8; 900];
        for attempt in 0..100u32 {
            tx_a.send(InternalServiceRequest::MediaSend {
                request_id: Uuid::new_v4(),
                cid: *cid_a,
                peer_cid: *cid_b,
                track: 0,
                kind: 0,
                timestamp: attempt,
                flags: 1,
                payload: payload.clone(),
            })
            .unwrap();
            match tokio::time::timeout(Duration::from_millis(200), rx_b.recv()).await {
                Ok(Some(InternalServiceResponse::MediaFrameNotification(frame))) => {
                    assert_eq!(frame.peer_cid, *cid_a);
                    assert_eq!(frame.payload, payload);
                    return;
                }
                Ok(Some(_)) | Err(_) => continue,
                Ok(None) => panic!("B's service channel closed"),
            }
        }
        panic!("no media frame crossed the peer connection");
    }

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

    /// 4. An expired config is no config. Relay-only through a dead relay would leave the pair
    ///    server-relayed; expired, it is ignored and the pair connects directly.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_expired_config_is_treated_as_none() -> Result<(), Box<dyn Error>> {
        setup_log();
        let expired = config(
            TurnPolicy::RelayOnly,
            vec![dead_turn_url()],
            "u",
            "p",
            unix_now() - 1,
        );
        let mut agents = two_registered_agents().await?;
        let paths = connect(&mut agents, [Some(expired.clone()), Some(expired)]).await?;
        assert_eq!(paths, [P2pPathReport::Direct, P2pPathReport::Direct]);
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

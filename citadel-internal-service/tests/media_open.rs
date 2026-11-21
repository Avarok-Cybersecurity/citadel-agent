use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::{
        get_free_port, open_media_and_measure, setup_log, two_sessions_on_one_service,
    };
    use citadel_internal_service_types::{
        InternalServiceRequest, InternalServiceResponse, P2pPathReport,
    };

    use citadel_sdk::prelude::*;
    use std::error::Error;
    use std::net::SocketAddr;
    use std::time::Duration;
    use uuid::Uuid;

    /// A media session opens over a peer connection brought up with UDP.
    ///
    /// The whole media path had NO Rust coverage: every harness helper passed
    /// `udp_mode: Default::default()`, and that default is `Disabled`, so no
    /// Rust test ever brought a peer connection up with UDP at all. Media was
    /// exercised only by the twelve-minute browser suite — which is why a bug
    /// as blunt as `UdpState::Pending` keeping one offer and dropping the other
    /// (dfb50a2) could only be caught in CI, and why backlog #56 spent rounds
    /// without a measurement of a successful negotiation.
    ///
    /// This does NOT reproduce #56: two services on loopback negotiate in
    /// milliseconds, and #56 is a timing race on CI's network. What it does is
    /// give the media open a regression test that runs in seconds, and print
    /// the negotiation time so the local figure can be compared against the
    /// `[UDP-NEGOTIATION]` line CI now emits.
    #[tokio::test]
    async fn a_media_session_opens_over_a_udp_enabled_peer_connection() -> Result<(), Box<dyn Error>>
    {
        setup_log();
        let addr_a: SocketAddr = format!("127.0.0.1:{}", get_free_port()).parse().unwrap();
        let addr_b: SocketAddr = format!("127.0.0.1:{}", get_free_port()).parse().unwrap();

        let mut peers = crate::common::register_and_connect_to_server_then_peers_with_udp::<
            StackedRatchet,
        >(vec![addr_a, addr_b], None, None, UdpMode::Enabled)
        .await?;
        let (peer_one, peer_two) = peers.as_mut_slice().split_at_mut(1_usize);
        let (to_service_a, from_service_a, cid_a) = peer_one.get_mut(0_usize).unwrap();
        let (_to_service_b, _from_service_b, cid_b) = peer_two.get_mut(0_usize).unwrap();

        open_media_and_measure(to_service_a, from_service_a, *cid_a, *cid_b, "two services").await;
        Ok(())
    }

    /// The same media open, with BOTH peers on ONE internal service.
    ///
    /// This is the topology the browser actually uses and the test above does
    /// not: one browser is one WebSocket is one internal service, hosting every
    /// session in that browser. The CI call failures (#56) all come from that
    /// shape, while the two-service test above passes in a millisecond -- so
    /// the difference is worth pinning rather than assuming it is immaterial.
    #[tokio::test]
    async fn a_media_session_opens_between_two_sessions_on_one_service(
    ) -> Result<(), Box<dyn Error>> {
        setup_log();
        let ((mut tx0, mut rx0, cid0), (mut tx1, mut rx1, cid1)) =
            two_sessions_on_one_service("intra").await?;

        crate::common::register_p2p(
            &mut tx0,
            &mut rx0,
            cid0,
            &mut tx1,
            &mut rx1,
            cid1,
            citadel_sdk::prelude::default_session_security_settings(),
            None::<PreSharedKey>,
        )
        .await?;
        crate::common::connect_p2p_with_udp(
            &mut tx0,
            &mut rx0,
            cid0,
            &mut tx1,
            &mut rx1,
            cid1,
            citadel_sdk::prelude::default_session_security_settings(),
            None::<PreSharedKey>,
            UdpMode::Enabled,
        )
        .await?;

        open_media_and_measure(&tx0, &mut rx0, cid0, cid1, "one service").await;
        Ok(())
    }

    /// The peer connection accepted with `PeerConnectAccept`, as the browser
    /// does it — and then a media open over it.
    ///
    /// No Rust test had ever sent `PeerConnectAccept`. The harness has BOTH
    /// sides send `PeerConnect`, a mutual connect that passes `udp_mode`
    /// explicitly on each side. The browser does not: one side connects and the
    /// other ACCEPTS, and `requests/peer/accept.rs` destructures `udp_mode: _`,
    /// relying entirely on the SDK reading the mode back out of the inbound
    /// `PeerSignal::PostConnect`.
    ///
    /// So the path CI exercises and the path the Rust tests exercise differ at
    /// exactly the field #56's failure message names. This pins the accepted
    /// path: if UDP does not survive it, a call over an accepted connection can
    /// never open, whatever `UDP_WAIT` is set to.
    #[tokio::test]
    async fn a_media_session_opens_over_an_accepted_peer_connection() -> Result<(), Box<dyn Error>>
    {
        setup_log();
        let ((tx0, mut rx0, cid0), (tx1, mut rx1, cid1)) =
            two_sessions_on_one_service("accept").await?;
        let (mut tx0m, mut tx1m) = (tx0.clone(), tx1.clone());

        crate::common::register_p2p(
            &mut tx0m,
            &mut rx0,
            cid0,
            &mut tx1m,
            &mut rx1,
            cid1,
            citadel_sdk::prelude::default_session_security_settings(),
            None::<PreSharedKey>,
        )
        .await?;

        // A connects, with UDP requested.
        tx0.send(InternalServiceRequest::PeerConnect {
            request_id: Uuid::new_v4(),
            cid: cid0,
            peer_cid: cid1,
            udp_mode: UdpMode::Enabled,
            session_security_settings: citadel_sdk::prelude::default_session_security_settings(),
            peer_session_password: None::<PreSharedKey>,
            turn: None,
        })
        .unwrap();

        // B is told, and its agent accepts: the account is hosted.
        let notification = tokio::time::timeout(Duration::from_secs(30), rx1.recv())
            .await
            .expect("no PeerConnectNotification within 30s")
            .expect("channel open");
        let InternalServiceResponse::PeerConnectNotification(notification) = notification else {
            panic!("expected a PeerConnectNotification, got {notification:?}")
        };
        assert_eq!(
            notification.udp_mode,
            UdpMode::Enabled,
            "the connect request's UdpMode did not survive the trip to the acceptor, so the \
             accepted connection could never carry media"
        );

        assert!(
            notification.answered_by_agent,
            "B's agent did not take the offer"
        );

        // The connector gets PeerConnectSuccess; the acceptor's window sent nothing.
        let signal = tokio::time::timeout(Duration::from_secs(30), rx0.recv())
            .await
            .expect("the connector never saw its connect result")
            .expect("channel open");
        let InternalServiceResponse::PeerConnectSuccess(s) = signal else {
            panic!("the connector got {signal:?} instead of a connect success")
        };
        // Media needs the datagram path, which attaches in the background: wait for it
        // (the agent's ensure_direct) before opening.
        let path =
            crate::common::settled_path(&mut rx0, s.cid, s.peer_cid, (s.path, s.upgrading)).await;
        assert_eq!(path, P2pPathReport::Direct, "connector");

        open_media_and_measure(&tx0, &mut rx0, cid0, cid1, "accepted connection").await;
        Ok(())
    }
}

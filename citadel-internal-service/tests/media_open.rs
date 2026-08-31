use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::{
        get_free_port, register_and_connect_to_server, setup_log, RegisterAndConnectItems,
    };
    use citadel_internal_service::kernel::CitadelWorkspaceService;
    use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
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

        let started = std::time::Instant::now();
        to_service_a
            .send(InternalServiceRequest::MediaOpen {
                request_id: Uuid::new_v4(),
                cid: *cid_a,
                peer_cid: *cid_b,
            })
            .unwrap();

        // Bounded: the failure this guards presents as silence, and an
        // unbounded recv would hang the suite instead of failing it.
        let response = tokio::time::timeout(Duration::from_secs(30), from_service_a.recv())
            .await
            .expect("no answer to MediaOpen within 30s")
            .expect("channel open");

        match response {
            InternalServiceResponse::MediaSessionOpened(opened) => {
                println!(
                    "MEASURED media open: {:?} (unreliable={}, max_frame={})",
                    started.elapsed(),
                    opened.unreliable,
                    opened.max_frame_bytes
                );
                assert_eq!(opened.peer_cid, *cid_b, "opened against the wrong peer");
                Ok(())
            }
            InternalServiceResponse::MediaSessionFailed(failed) => {
                // The message is the diagnostic: it names whether no UDP channel
                // arrived in the budget, or the connection had UdpMode disabled.
                panic!(
                    "media open failed after {:?}: {}",
                    started.elapsed(),
                    failed.message
                )
            }
            other => panic!("expected a media session result, got {other:?}"),
        }
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
        let (server, server_bind_address) =
            crate::common::server_info_skip_cert_verification::<StackedRatchet>();
        tokio::task::spawn(server);

        let service_addr: SocketAddr = format!("127.0.0.1:{}", get_free_port()).parse().unwrap();
        let service = CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(service_addr).await?;
        let internal_service = NodeBuilder::default()
            .with_backend(BackendType::InMemory)
            .with_node_type(NodeType::Peer)
            .with_insecure_skip_cert_verification()
            .build(service)?;
        tokio::task::spawn(internal_service);
        tokio::time::sleep(Duration::from_millis(1000)).await;

        let to_spawn = vec![
            RegisterAndConnectItems {
                internal_service_addr: service_addr,
                server_addr: server_bind_address,
                full_name: "Peer 0".to_string(),
                username: "peer.0".to_string(),
                password: "secret_0".to_string().into_bytes().to_owned(),
                pre_shared_key: None::<PreSharedKey>,
            },
            RegisterAndConnectItems {
                internal_service_addr: service_addr,
                server_addr: server_bind_address,
                full_name: "Peer 1".to_string(),
                username: "peer.1".to_string(),
                password: "secret_1".to_string().into_bytes().to_owned(),
                pre_shared_key: None::<PreSharedKey>,
            },
        ];

        let mut info = register_and_connect_to_server(to_spawn).await.unwrap();
        let (mut tx0, mut rx0, cid0) = info.remove(0);
        let (mut tx1, mut rx1, cid1) = info.remove(0);

        crate::common::register_p2p(
            &mut tx0,
            &mut rx0,
            cid0,
            &mut tx1,
            &mut rx1,
            cid1,
            SessionSecuritySettings::default(),
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
            SessionSecuritySettings::default(),
            None::<PreSharedKey>,
            UdpMode::Enabled,
        )
        .await?;

        let started = std::time::Instant::now();
        tx0.send(InternalServiceRequest::MediaOpen {
            request_id: Uuid::new_v4(),
            cid: cid0,
            peer_cid: cid1,
        })
        .unwrap();

        let response = tokio::time::timeout(Duration::from_secs(30), rx0.recv())
            .await
            .expect("no answer to MediaOpen within 30s")
            .expect("channel open");

        match response {
            InternalServiceResponse::MediaSessionOpened(opened) => {
                println!(
                    "MEASURED intra-service media open: {:?} (unreliable={})",
                    started.elapsed(),
                    opened.unreliable
                );
                Ok(())
            }
            InternalServiceResponse::MediaSessionFailed(failed) => panic!(
                "media open failed after {:?}: {}",
                started.elapsed(),
                failed.message
            ),
            other => panic!("expected a media session result, got {other:?}"),
        }
    }
}

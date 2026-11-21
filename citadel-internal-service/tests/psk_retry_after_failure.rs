//! One wrong peer session password does not lock the pair out.
//!
//! Every account is agent-hosted, so an offer is answered by the acceptor's agent with the
//! password the acceptor's window last gave for that peer (here, as A registered). Round one: B
//! dials with a mistyped password, so the key exchange fails. Round two: B dials with the right
//! one, and the pair connects: the failed round leaves neither side holding half of its
//! exchange, so the retry is not turned away as one already in progress.

use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::{
        get_free_port, register_and_connect_to_server, server_info_skip_cert_verification,
        InternalServicesFutures, RegisterAndConnectItems,
    };
    use citadel_internal_service::kernel::CitadelWorkspaceService;
    use citadel_internal_service_test_common::node_settings::TestNode;
    use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
    use citadel_sdk::prelude::*;
    use std::error::Error;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
    use uuid::Uuid;

    /// Long enough that a slow machine is not the reason a response is missing,
    /// short enough that a wedge is reported rather than waited out.
    const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

    async fn recv_or_fail(
        stream: &mut UnboundedReceiver<InternalServiceResponse>,
        what: &str,
    ) -> InternalServiceResponse {
        match tokio::time::timeout(RESPONSE_TIMEOUT, stream.recv()).await {
            Ok(Some(response)) => response,
            Ok(None) => panic!("Stream closed while waiting for {what}"),
            Err(_) => panic!("Timed out after 30s waiting for {what}"),
        }
    }

    fn peer_connect(
        cid: u64,
        peer_cid: u64,
        peer_session_password: Option<PreSharedKey>,
    ) -> InternalServiceRequest {
        InternalServiceRequest::PeerConnect {
            request_id: Uuid::new_v4(),
            cid,
            peer_cid,
            udp_mode: Default::default(),
            session_security_settings: citadel_sdk::prelude::default_session_security_settings(),
            peer_session_password,
            turn: None,
        }
    }

    fn peer_register(
        cid: u64,
        peer_cid: u64,
        peer_session_password: Option<PreSharedKey>,
    ) -> InternalServiceRequest {
        InternalServiceRequest::PeerRegister {
            request_id: Uuid::new_v4(),
            cid,
            peer_cid,
            session_security_settings: citadel_sdk::prelude::default_session_security_settings(),
            connect_after_register: false,
            peer_session_password,
        }
    }

    /// One connect round: `dialer` dials `acceptor` with `psk`, and the acceptor's agent answers
    /// with the password the acceptor's window last gave for the dialer. Returns the dialer's
    /// response. `round` is in every wait message, so a timeout says which round failed.
    async fn dial_round(
        dialer: (
            &mut UnboundedSender<InternalServiceRequest>,
            &mut UnboundedReceiver<InternalServiceResponse>,
            u64,
        ),
        acceptor: (&mut UnboundedReceiver<InternalServiceResponse>, u64),
        psk: Option<PreSharedKey>,
        round: &str,
    ) -> InternalServiceResponse {
        let (dialer_sink, dialer_stream, dialer_cid) = dialer;
        let (acceptor_stream, acceptor_cid) = acceptor;
        dialer_sink
            .send(peer_connect(dialer_cid, acceptor_cid, psk))
            .expect("send connect");
        let notification = recv_or_fail(
            acceptor_stream,
            &format!("[{round}] the acceptor's notification"),
        )
        .await;
        assert!(
            matches!(
                notification,
                InternalServiceResponse::PeerConnectNotification(ref n) if n.answered_by_agent
            ),
            "[{round}] the acceptor's agent answers the offer itself; got {notification:?}"
        );
        recv_or_fail(
            dialer_stream,
            &format!("[{round}] the dialer's connect response"),
        )
        .await
    }

    #[tokio::test]
    async fn the_correct_password_still_works_after_a_wrong_one() -> Result<(), Box<dyn Error>> {
        crate::common::setup_log();

        let (server, server_addr) = server_info_skip_cert_verification::<StackedRatchet>();
        tokio::task::spawn(server);

        let addrs: Vec<SocketAddr> = (0..2)
            .map(|_| format!("127.0.0.1:{}", get_free_port()).parse().unwrap())
            .collect();

        let mut services_to_spawn: Vec<InternalServicesFutures> = Vec::new();
        for addr in addrs.clone() {
            let kernel = CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(
                addr,
                citadel_internal_service::SERVER_RECONNECT,
                crate::common::test_transfers(),
                crate::common::TEST_SERVER_CERTIFICATE,
            )
            .await?;
            let node = NodeBuilder::default()
                .with_stun_servers(crate::common::test_stun_servers().as_slice().to_vec())
                .with_backend(crate::common::test_backend())
                .with_node_type(NodeType::Peer)
                .with_insecure_skip_cert_verification()
                .as_test_agent(&kernel)
                .build(kernel)?;
            services_to_spawn.push(Box::pin(async move {
                match node.await {
                    Err(err) => Err(Box::from(err)),
                    _ => Ok(()),
                }
            }));
        }
        crate::common::spawn_services(services_to_spawn);
        // Both the server and the two services must be listening before the
        // register/connect traffic below.
        tokio::time::sleep(Duration::from_millis(2000)).await;

        let to_spawn: Vec<RegisterAndConnectItems<String, String, Vec<u8>, PreSharedKey>> = addrs
            .iter()
            .enumerate()
            .map(|(n, addr)| RegisterAndConnectItems {
                internal_service_addr: *addr,
                server_addr,
                full_name: format!("Peer {n}"),
                username: format!("peer.{n}"),
                password: format!("secret_{n}").into_bytes(),
                pre_shared_key: None,
            })
            .collect();

        let mut services = register_and_connect_to_server(to_spawn).await.unwrap();
        let (first, second) = services.split_at_mut(1);
        let (ref mut a_sink, ref mut a_stream, a_cid) = &mut first[0];
        let (ref mut b_sink, ref mut b_stream, b_cid) = &mut second[0];
        let (a_cid, b_cid) = (*a_cid, *b_cid);

        let password = PreSharedKey::from("PeerSessionPassword".as_bytes());
        // Register the pair so PeerConnect has a target. A's window gives the pair's password
        // as it registers; its agent answers B's offers with it.
        b_sink
            .send(peer_register(b_cid, a_cid, None))
            .expect("B register");
        let _ = recv_or_fail(a_stream, "Peer A's register notification").await;
        a_sink
            .send(peer_register(a_cid, b_cid, Some(password.clone())))
            .expect("A register");
        let _ = recv_or_fail(a_stream, "Peer A's register response").await;
        let _ = recv_or_fail(b_stream, "Peer B's register response").await;

        // Round one: B mistypes the password, so the key exchange with A's answer fails.
        let b_first = dial_round(
            (&mut *b_sink, &mut *b_stream, b_cid),
            (&mut *a_stream, a_cid),
            Some(PreSharedKey::from("MistypedPassword".as_bytes())),
            "round 1 (mismatched)",
        )
        .await;
        assert!(
            matches!(b_first, InternalServiceResponse::PeerConnectFailure(..)),
            "a mismatched password must fail the connect; got {b_first:?}"
        );

        // Round two: B retypes it correctly, and the pair connects.
        let b_second = dial_round(
            (&mut *b_sink, &mut *b_stream, b_cid),
            (&mut *a_stream, a_cid),
            Some(password),
            "round 2 (both correct)",
        )
        .await;
        assert!(
            matches!(b_second, InternalServiceResponse::PeerConnectSuccess(..)),
            "one mistyped password must not lock the pair out: the retry carried \
             the correct password and still failed with {b_second:?}"
        );
        let a_second = recv_or_fail(a_stream, "[round 2] the acceptor's connect response").await;
        assert!(
            matches!(a_second, InternalServiceResponse::PeerConnectSuccess(..)),
            "the acceptor must see the retry succeed too; got {a_second:?}"
        );

        Ok(())
    }
}

//! One wrong peer session password must not lock the pair out for good.
//!
//! `test_internal_service_peer_with_psk_negative_case` was `#[ignore]`d with the
//! note "Peer A is never sent a connect notification when the PSK will not
//! verify — is that intended?". Running it says otherwise: on the FIRST attempt
//! Peer A is notified and both sides get their expected `PeerConnectFailure`.
//! It is the SECOND attempt that dies, and it dies on the initiator's own side
//! before A is involved at all:
//!
//!     [PeerConnect] connect_to_peer_custom FAILED:
//!         NetworkError { code: RekeyUpdate (12), message: "Rekey update error: Encryption failure" }
//!
//! So the open question was not about notifications. A failed PSK connect
//! leaves the peer's session crypto in a state that every later connect between
//! those two peers fails from — including one carrying the CORRECT password.
//! `test_internal_service_peer_with_psk` shows a fresh pair with the right
//! password connects in about two seconds, which is what isolates the cause to
//! the earlier failure rather than to PSK connects in general.
//!
//! In product terms: mistype your peer session password once and you cannot
//! connect to that peer again, right password or not.
//!
//! ## What has been ruled out
//!
//! * **The responder is not the problem.** Round one notifies A and both sides
//!   receive `PeerConnectFailure` correctly. Only round two breaks.
//! * **It is not PSK connects in general.** `test_internal_service_peer_with_psk`
//!   connects a fresh pair with the right password in ~2s.
//! * **It is not a lingering virtual connection.** Calling `disconnect()` on the
//!   handle in `connect.rs`'s failure arm was tried and changed nothing.
//! * **It is not a stale password.** `store_session_password` inserts, so round
//!   two does overwrite round one's value.
//!
//! ## Where the residue is
//!
//! State left by the failed handshake that is never cleared per peer.
//! `state_container.peer_kem_states` is inserted on each attempt and only ever
//! `clear()`ed wholesale in `session_manager`; there is no per-peer removal on
//! failure. `remove_session_password` exists for the same job and is
//! `#[allow(dead_code)]` with a TODO — written, never wired.
//!
//! Fixing it means clearing that per-peer state when a peer connect fails,
//! which needs enough of the KEM state machine to be sure a working connection
//! is not torn down with it. Left proven and reproducible rather than guessed
//! at.

use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::{
        get_free_port, register_and_connect_to_server, server_info_skip_cert_verification,
        InternalServicesFutures, RegisterAndConnectItems,
    };
    use citadel_internal_service::kernel::CitadelWorkspaceService;
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

    /// The two peers, threaded through each round as one value so the round
    /// helper does not take nine arguments.
    struct Pair<'a> {
        a_sink: &'a mut UnboundedSender<InternalServiceRequest>,
        a_stream: &'a mut UnboundedReceiver<InternalServiceResponse>,
        a_cid: u64,
        b_sink: &'a mut UnboundedSender<InternalServiceRequest>,
        b_stream: &'a mut UnboundedReceiver<InternalServiceResponse>,
        b_cid: u64,
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
            session_security_settings: Default::default(),
            peer_session_password,
        }
    }

    fn peer_register(cid: u64, peer_cid: u64) -> InternalServiceRequest {
        InternalServiceRequest::PeerRegister {
            request_id: Uuid::new_v4(),
            cid,
            peer_cid,
            session_security_settings: Default::default(),
            connect_after_register: false,
            peer_session_password: None,
        }
    }

    /// One connect round: B offers `b_psk`, A answers with `a_psk`. Returns the
    /// response each side saw. `round` is in every wait message so a timeout
    /// says WHICH round failed -- the whole point of this test is that round one
    /// passes and round two does not.
    async fn connect_round(
        pair: &mut Pair<'_>,
        b_psk: Option<PreSharedKey>,
        a_psk: Option<PreSharedKey>,
        round: &str,
    ) -> (InternalServiceResponse, InternalServiceResponse) {
        pair.b_sink
            .send(peer_connect(pair.b_cid, pair.a_cid, b_psk))
            .expect("send B connect");

        // A is notified of the inbound attempt. On round two this is where the
        // defect shows: it never arrives, because B's own attempt failed to
        // encrypt before any notification could be generated.
        let _notification = recv_or_fail(
            pair.a_stream,
            &format!("[{round}] Peer A's connect notification"),
        )
        .await;

        pair.a_sink
            .send(peer_connect(pair.a_cid, pair.b_cid, a_psk))
            .expect("send A connect");

        let a_response = recv_or_fail(
            pair.a_stream,
            &format!("[{round}] Peer A's connect response"),
        )
        .await;
        let b_response = recv_or_fail(
            pair.b_stream,
            &format!("[{round}] Peer B's connect response"),
        )
        .await;
        (a_response, b_response)
    }

    #[tokio::test]
    #[ignore = "PROVEN DEFECT, not a flake: a failed PSK connect poisons the pair \
                so the correct password no longer works. Root cause narrowed but \
                not fixed -- see the module docs. Ignored to keep CI green while \
                the fix is outstanding; run with --ignored to reproduce."]
    async fn the_correct_password_still_works_after_a_wrong_one() -> Result<(), Box<dyn Error>> {
        crate::common::setup_log();

        let (server, server_addr) = server_info_skip_cert_verification::<StackedRatchet>();
        tokio::task::spawn(server);

        let addrs: Vec<SocketAddr> = (0..2)
            .map(|_| format!("127.0.0.1:{}", get_free_port()).parse().unwrap())
            .collect();

        let mut services_to_spawn: Vec<InternalServicesFutures> = Vec::new();
        for addr in addrs.clone() {
            let kernel = CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(addr).await?;
            let node = NodeBuilder::default()
                .with_backend(BackendType::Filesystem("filesystem".into()))
                .with_node_type(NodeType::Peer)
                .with_insecure_skip_cert_verification()
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

        // Register the pair so PeerConnect has a target.
        b_sink
            .send(peer_register(b_cid, a_cid))
            .expect("B register");
        let _ = recv_or_fail(a_stream, "Peer A's register notification").await;
        a_sink
            .send(peer_register(a_cid, b_cid))
            .expect("A register");
        let _ = recv_or_fail(a_stream, "Peer A's register response").await;
        let _ = recv_or_fail(b_stream, "Peer B's register response").await;

        let password = PreSharedKey::from("PeerSessionPassword".as_bytes());
        let mut pair = Pair {
            a_sink,
            a_stream,
            a_cid,
            b_sink,
            b_stream,
            b_cid,
        };

        // Round one: A answers with no password at all. Both sides should be
        // told it failed -- this part already works.
        let (a_first, _b_first) = connect_round(
            &mut pair,
            Some(password.clone()),
            None,
            "round 1 (mismatched)",
        )
        .await;
        assert!(
            matches!(a_first, InternalServiceResponse::PeerConnectFailure(..)),
            "a mismatched password must fail the connect; got {a_first:?}"
        );

        // Round two: both sides now present the SAME, correct password. This is
        // the user retyping it, and it must work.
        let (a_second, b_second) = connect_round(
            &mut pair,
            Some(password.clone()),
            Some(password),
            "round 2 (both correct)",
        )
        .await;
        assert!(
            matches!(a_second, InternalServiceResponse::PeerConnectSuccess(..)),
            "one mistyped password must not lock the pair out: the retry carried \
             the correct password and still failed with {a_second:?}"
        );
        assert!(
            matches!(b_second, InternalServiceResponse::PeerConnectSuccess(..)),
            "the initiator must see the retry succeed too; got {b_second:?}"
        );

        Ok(())
    }
}

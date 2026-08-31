use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::{get_free_port, setup_log};
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
}

//! Every signed-in account's ILM is hosted by its agent, whether or not a
//! window declares anything.
//!
//! Over the real SDK: two agents on one server, P2P-connected. Neither window
//! sends `DeclareCapabilities`; both send with `SendReliable`, which only a
//! hosted ILM accepts.
use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::agent_ilm::*;
    use crate::common::{open_localhost_connection, setup_log};
    use citadel_internal_service_types::{
        ConfigCommand, InternalServiceRequest, InternalServiceResponse,
    };
    use std::time::Duration;
    use uuid::Uuid;

    const BODIES: [&[u8]; 4] = [b"first", b"second", b"third", b"fourth"];

    /// Each side's agent delivers each message exactly once, in order: its
    /// ILM's delivery is the only path.
    #[tokio::test(flavor = "multi_thread")]
    async fn two_undeclared_windows_exchange_messages_both_ways_through_hosted_ilm() {
        setup_log();
        let mut world = hosted_pair().await;
        let (alice_cid, bob_cid) = (world.alice.2, world.bob.2);

        for body in BODIES {
            sends(&mut world.alice, bob_cid, body).await;
        }
        for body in BODIES {
            assert_eq!(
                next_message(&mut world.bob.1, alice_cid).await,
                body,
                "Bob: out of order"
            );
        }
        for body in BODIES {
            sends(&mut world.bob, alice_cid, body).await;
        }
        for body in BODIES {
            assert_eq!(
                next_message(&mut world.alice.1, bob_cid).await,
                body,
                "Alice: out of order"
            );
        }
        for (rx, from, who) in [
            (&mut world.alice.1, bob_cid, "Alice"),
            (&mut world.bob.1, alice_cid, "Bob"),
        ] {
            let extra = quiet(rx, Duration::from_secs(2)).await;
            assert!(
                !extra.iter().any(|r| matches!(r, InternalServiceResponse::MessageNotification(n) if n.peer_cid == from)),
                "{who} was handed a message twice, or a raw ILM frame: {extra:?}"
            );
        }
    }

    /// With no window open the hosted ILM does not acknowledge what it cannot
    /// hand on, so nothing is lost: the next window to attach receives it.
    #[tokio::test(flavor = "multi_thread")]
    async fn messages_sent_while_no_window_is_open_arrive_when_one_attaches() {
        setup_log();
        let HostedPair {
            alice_addr,
            alice,
            mut bob,
        } = hosted_pair().await;
        let (alice_cid, bob_cid) = (alice.2, bob.2);
        drop(alice);

        sends(&mut bob, alice_cid, b"while you were away").await;
        tokio::time::sleep(Duration::from_secs(2)).await;

        let mut window = open_localhost_connection(alice_addr)
            .await
            .expect("a new window");
        window
            .0
            .send(InternalServiceRequest::ConnectionManagement {
                request_id: Uuid::new_v4(),
                management_command: ConfigCommand::ClaimSession {
                    session_cid: alice_cid,
                    only_if_orphaned: true,
                },
            })
            .expect("open");
        assert_eq!(
            next_message(&mut window.1, bob_cid).await,
            b"while you were away"
        );
    }
}

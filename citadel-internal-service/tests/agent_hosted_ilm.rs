//! An account whose ILM the agent hosts, talking to one whose window runs ILM
//! itself -- what a peer on an older agent, or an older UI, is.
//!
//! Over the real SDK: two agents on one server, P2P-connected. Alice's window
//! declares `agent_ilm` and sends with `SendReliable`; Bob's window is the
//! connector's own messenger, the code a browser runs.
use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::agent_ilm::*;
    use crate::common::{open_localhost_connection, setup_log};
    use citadel_internal_service_connector::messenger::{
        IlmOptions, InternalMessage, ACCOUNT_ILM_OPTIONS,
    };
    use citadel_internal_service_types::{
        ConfigCommand, InternalServiceRequest, InternalServiceResponse,
    };
    use serde::Deserialize;
    use std::time::Duration;
    use uuid::Uuid;

    const BODIES: [&[u8]; 4] = [b"first", b"second", b"third", b"fourth"];

    fn first_copies(seen: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
        // The browser's messenger hands its UI each message twice (an immediate
        // forward and ILM's delivery) and the UI de-duplicates; do the same.
        let mut out: Vec<Vec<u8>> = Vec::new();
        for body in seen {
            if !out.contains(&body) {
                out.push(body);
            }
        }
        out
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn agent_hosted_and_browser_ilm_exchange_messages_both_ways() {
        setup_log();
        let mut world = mixed_hosting(ACCOUNT_ILM_OPTIONS).await;
        let (alice_cid, bob_cid) = (world.alice.2, world.bob.cid);

        for body in BODIES {
            alice_sends(&mut world.alice, bob_cid, body).await;
        }
        let mut at_bob = Vec::new();
        while first_copies(at_bob.clone()).len() < BODIES.len() {
            at_bob.push(next_message(&mut world.bob.rx, alice_cid).await);
        }
        assert_eq!(
            first_copies(at_bob),
            BODIES.map(|b| b.to_vec()),
            "Bob: out of order or altered"
        );

        for body in BODIES {
            world
                .bob
                .tx
                .send_message_to(alice_cid, body.to_vec())
                .await
                .expect("bob sends");
        }
        // Alice's agent delivers each exactly once: ILM's delivery is the only path.
        for body in BODIES {
            assert_eq!(
                next_message(&mut world.alice.1, bob_cid).await,
                body,
                "Alice: out of order"
            );
        }
        let extra = quiet(&mut world.alice.1, Duration::from_secs(2)).await;
        assert!(
            !extra.iter().any(|r| matches!(r, InternalServiceResponse::MessageNotification(n) if n.peer_cid == bob_cid)),
            "Alice was handed a message twice, or a raw ILM frame: {extra:?}"
        );
    }

    /// `WireWrapper` as builds before the extensions have it.
    #[derive(Deserialize)]
    #[allow(dead_code)]
    enum Legacy {
        Message {
            source: u64,
            destination: u64,
            message_id: u64,
            contents: Vec<u8>,
        },
        ISMAux {
            signal: Box<InternalMessage>,
        },
    }

    /// A peer that predates piggybacked ACKs and compression never receives a
    /// frame it cannot read from an agent-hosted ILM: the agent's frames are the
    /// browser's frames, extensions only after the peer advertised them.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_agent_hosted_ilm_never_sends_a_legacy_peer_a_frame_it_cannot_read() {
        setup_log();
        let mut world = mixed_hosting(IlmOptions::LEGACY).await;
        let (alice_cid, bob_cid) = (world.alice.2, world.bob.cid);
        // Long and repetitive: what would be compressed toward a capable peer.
        let big = "a compressible line of text ".repeat(200).into_bytes();
        alice_sends(&mut world.alice, bob_cid, &big).await;
        assert_eq!(next_message(&mut world.bob.rx, alice_cid).await, big);
        world
            .bob
            .tx
            .send_message_to(alice_cid, b"ack me".to_vec())
            .await
            .expect("bob sends");
        assert_eq!(next_message(&mut world.alice.1, bob_cid).await, b"ack me");

        let arrived = world.bob.arrived.lock().expect("lock").clone();
        assert!(
            !arrived.is_empty(),
            "Bob saw no frames; the check proves nothing"
        );
        let unreadable = arrived
            .iter()
            .filter(|bytes| bincode2::deserialize::<Legacy>(bytes).is_err())
            .count();
        assert_eq!(
            unreadable, 0,
            "a legacy peer was sent {unreadable} frame(s) it cannot decode"
        );
    }

    /// With no window open the hosted ILM does not acknowledge what it cannot
    /// hand on, so nothing is lost: the next window to attach receives it.
    #[tokio::test(flavor = "multi_thread")]
    async fn messages_sent_while_no_window_is_open_arrive_when_one_attaches() {
        setup_log();
        let world = mixed_hosting(ACCOUNT_ILM_OPTIONS).await;
        let MixedHosting {
            alice_addr,
            alice,
            bob,
        } = world;
        let (alice_cid, bob_cid) = (alice.2, bob.cid);
        drop(alice);

        bob.tx
            .send_message_to(alice_cid, b"while you were away".to_vec())
            .await
            .expect("bob sends");
        tokio::time::sleep(Duration::from_secs(2)).await;

        let mut window = open_localhost_connection(alice_addr)
            .await
            .expect("a new window");
        declared(&mut window).await;
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

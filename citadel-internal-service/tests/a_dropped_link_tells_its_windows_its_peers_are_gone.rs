//! When a session's link to its server drops, its peer connections go with it, and every
//! window attached to the session is told so, one `DisconnectNotification` per peer.
//!
//! Seen live (an agent on a laptop moving between cell towers): the reconnect cleared the
//! session's peers without a word, so its windows went on showing them connected. The
//! UI's auto-connect skipped them as "Already connected" on every poll, and the P2P links
//! came back only when the remote side happened to redial.
//!
//! The proxy strands only this session's link (reset on the agent's side, left open on
//! the server's), as an IP change does; the peer reaches the server directly.

#[allow(dead_code)]
#[path = "reconnect_support/mod.rs"]
mod reconnect;
#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service_test_common as common;
use citadel_internal_service_types::{DisconnectNotification, InternalServiceResponse};
use citadel_sdk::prelude::*;
use common::group::recv_until;
use reconnect::Proxy;
use std::error::Error;

#[tokio::test]
async fn a_dropped_link_tells_its_windows_its_peers_are_gone() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let (server, server_addr) = common::server_info_skip_cert_verification::<StackedRatchet>();
    tokio::spawn(server);
    let proxy = Proxy::start(server_addr).await?;
    let (_, (mut tx_a, mut rx_a, cid_a), (mut tx_b, mut rx_b, cid_b)) =
        common::two_sessions_on_one_service_reaching("lost-peers", [proxy.addr, server_addr])
            .await?;
    common::register_p2p(
        &mut tx_a,
        &mut rx_a,
        cid_a,
        &mut tx_b,
        &mut rx_b,
        cid_b,
        SessionSecuritySettings::default(),
        None::<PreSharedKey>,
    )
    .await?;
    common::connect_p2p(
        &mut tx_a,
        &mut rx_a,
        cid_a,
        &mut tx_b,
        &mut rx_b,
        cid_b,
        SessionSecuritySettings::default(),
        None::<PreSharedKey>,
    )
    .await?;

    proxy.strand();

    let told = recv_until(
        &mut rx_a,
        "A's window hears its peer is gone",
        |r| matches!(r, InternalServiceResponse::DisconnectNotification(n) if n.peer_cid.is_some()),
    )
    .await;
    let InternalServiceResponse::DisconnectNotification(DisconnectNotification {
        cid,
        peer_cid,
        ..
    }) = told
    else {
        unreachable!("matched above")
    };
    assert_eq!(cid, cid_a, "addressed to the session whose link dropped");
    assert_eq!(peer_cid, Some(cid_b), "naming the peer it lost");
    Ok(())
}

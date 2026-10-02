//! A server that never saw the link die keeps the session -- up to an hour, until its
//! keep-alive check ends it -- and refused every reconnect as "Session Already Connected"
//! all that time. Measured live on a laptop moving between cell towers: ten refused
//! attempts 30 s apart after `No route to host`, while its peer timed out redialling.
//!
//! The server now issues each session a resume token and replaces the session it holds
//! when the same client's reconnect presents it, after the credentials check
//! (Citadel-Protocol protocol 0.11.2, `SESSION_RESUME_SINCE`). The agent's reconnect still
//! never forces: only its own dead session can be replaced this way.
//!
//! The proxy strands the server's end of the link (reset on the agent's side only) and
//! never releases it, so the server holds the session for the whole test. A reconnect
//! that comes back here can only have replaced it.

// Shared with server_reconnect.rs, which uses the helpers this file does not.
#[allow(dead_code)]
#[path = "reconnect_support/mod.rs"]
mod reconnect;
#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service::{ReconnectPolicy, SERVER_RECONNECT};
use citadel_internal_service_test_common as common;
use citadel_internal_service_types::InternalServiceResponse;
use citadel_sdk::prelude::*;
use reconnect::{list_peers, next_link_event, Proxy};
use std::error::Error;
use std::time::Duration;
use support::{open, register, session, spawn_agent_with, temp_store, username};

/// Seconds where the agent's is ten minutes, so a reconnect that was refused would give up
/// inside the test instead of coming back.
const QUICK: ReconnectPolicy = ReconnectPolicy {
    first_delay: Duration::from_millis(500),
    max_delay: Duration::from_secs(2),
    give_up_after: Duration::from_secs(8),
    attempt_timeout: Duration::from_secs(10),
    server_holds_session_for: Duration::ZERO,
};

#[tokio::test]
async fn the_reconnect_replaces_the_dead_session_the_server_holds() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let (server, server_addr) = common::server_info_skip_cert_verification::<StackedRatchet>();
    tokio::spawn(server);
    let proxy = Proxy::start(server_addr).await?;
    let store = temp_store();
    let (agent, _node) = spawn_agent_with(&store, QUICK).await?;
    let (mut sink, mut stream) = open(agent).await?;
    let cid = register(&mut sink, &mut stream, &proxy.addr.to_string(), &username()).await??;

    proxy.strand();
    assert_eq!(
        next_link_event(&mut stream, cid).await?,
        "lost(reconnecting=true)"
    );
    assert_eq!(
        next_link_event(&mut stream, cid).await?,
        "reconnected",
        "the reconnect did not replace the session the server still holds"
    );
    assert_eq!(session(&mut sink, &mut stream, cid).await?.cid, cid);
    let answer = list_peers(&mut sink, &mut stream, cid).await?;
    assert!(
        matches!(answer, InternalServiceResponse::ListAllPeersResponse(ref r) if r.cid == cid),
        "the server answers over the new link: {answer:?}"
    );
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

//! A server that never saw the link die keeps the session, and refuses every reconnect as
//! "Session Already Connected" until its keep-alive check ends it -- up to an hour. The
//! agent gave up after ten minutes and removed the session, so a user lost a session the
//! server was about to let go of, and nothing brought it back. Measured live on a hosted
//! workspace: 24 refused attempts on the ten-minute schedule, then the account's chip gone.
//!
//! The proxy strands the server's end of the link (reset on the agent's side only) and
//! releases it later, which is what the server's keep-alive check does, just sooner. The
//! agent runs a policy whose ordinary limit is seconds, so a reconnect that treated the
//! refusal as any other failure gives up inside the test.

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
use reconnect::{link_events, list_peers, next_link_event, Proxy};
use std::error::Error;
use std::time::Duration;
use support::{open, register, session, spawn_agent_with, temp_store, username};

/// Seconds where the agent's is ten minutes; how long the server may hold a dead session
/// stays the SDK's.
const QUICK: ReconnectPolicy = ReconnectPolicy {
    first_delay: Duration::from_millis(500),
    max_delay: Duration::from_secs(2),
    give_up_after: Duration::from_secs(8),
    attempt_timeout: Duration::from_secs(10),
    ..SERVER_RECONNECT
};

#[tokio::test]
async fn a_server_still_holding_the_dead_session_is_waited_out() -> Result<(), Box<dyn Error>> {
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

    // Well past the ordinary limit, every attempt refused as "already connected".
    let meanwhile = link_events(
        &mut stream,
        cid,
        QUICK.give_up_after + Duration::from_secs(6),
    )
    .await;
    assert!(
        meanwhile.is_empty(),
        "the agent gave up on a session the server still holds: {meanwhile:?}"
    );
    assert_eq!(
        session(&mut sink, &mut stream, cid).await?.cid,
        cid,
        "the session is still offered while it is being reconnected"
    );

    // The server lets its dead session go; the next attempt is admitted.
    proxy.release();
    assert_eq!(next_link_event(&mut stream, cid).await?, "reconnected");
    let answer = list_peers(&mut sink, &mut stream, cid).await?;
    assert!(
        matches!(answer, InternalServiceResponse::ListAllPeersResponse(ref r) if r.cid == cid),
        "the server answers over the new link: {answer:?}"
    );
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

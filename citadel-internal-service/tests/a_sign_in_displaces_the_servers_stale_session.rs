//! A user who signs in while the server still holds their dead session gets in at once.
//!
//! Before Citadel-Protocol #319 the server refused every login for the account ("Session
//! Already Connected") until its keep-alive ended the dead session -- up to an hour after a
//! one-sided drop. Now a login with `force_login` that fully authenticates replaces it, and the
//! agent sends `force_login` for a user's sign-in (requests/connect_mode.rs).
//!
//! The proxy strands the server's end of the link and never releases it, so the only way in is
//! the replacement. The agent's own reconnect is still running when the user signs in; the
//! session must stay live after that reconnect's ordinary limit has passed.

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
use reconnect::{
    connect_observing, connect_with_password, link_events, list_peers, next_link_event, Proxy,
};
use std::error::Error;
use std::time::Duration;
use support::{open, register, spawn_agent_with, temp_store, username, PASSWORD};

/// Seconds where the agent's is ten minutes, so the background reconnect's limit passes inside
/// the test.
const QUICK: ReconnectPolicy = ReconnectPolicy {
    first_delay: Duration::from_millis(500),
    max_delay: Duration::from_secs(2),
    give_up_after: Duration::from_secs(8),
    attempt_timeout: Duration::from_secs(10),
    ..SERVER_RECONNECT
};

#[tokio::test]
async fn a_sign_in_replaces_the_session_the_server_still_holds() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let (server, server_addr) = common::server_info_skip_cert_verification::<StackedRatchet>();
    tokio::spawn(server);
    let proxy = Proxy::start(server_addr).await?;
    let store = temp_store();
    let (agent, _node) = spawn_agent_with(&store, QUICK).await?;
    let (mut sink, mut stream) = open(agent).await?;
    let name: String = username();
    let cid = register(&mut sink, &mut stream, &proxy.addr.to_string(), &name).await??;

    proxy.strand(); // never released: the server holds the dead session for the whole test
    assert_eq!(
        next_link_event(&mut stream, cid).await?,
        "lost(reconnecting=true)"
    );

    let started = std::time::Instant::now();
    let (answer, told) = connect_observing(&mut sink, &mut stream, &name, PASSWORD).await?;
    assert!(
        matches!(answer, InternalServiceResponse::ConnectSuccess(ref s) if s.cid == cid),
        "the sign-in was refused while the server held the dead session: {answer:?}"
    );
    assert_eq!(
        told,
        ["reconnected"],
        "the page is told the session is back, as a reconnect would tell it"
    );
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "the sign-in waited {:?}, as if for the server's keep-alive",
        started.elapsed()
    );

    // The agent's background reconnect passes its ordinary limit; the new session survives
    // it: nothing is lost, failed or ended, and the server still answers over the session.
    let later = link_events(
        &mut stream,
        cid,
        QUICK.give_up_after + Duration::from_secs(4),
    )
    .await;
    assert!(
        later.is_empty(),
        "the stopped reconnect disturbed the session the user signed in to: {later:?}"
    );
    let peers = list_peers(&mut sink, &mut stream, cid).await?;
    assert!(
        matches!(peers, InternalServiceResponse::ListAllPeersResponse(ref r) if r.cid == cid),
        "the session the user signed in to did not survive the old reconnect: {peers:?}"
    );
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

#[tokio::test]
async fn a_wrong_password_leaves_the_reconnect_running() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let (server, server_addr) = common::server_info_skip_cert_verification::<StackedRatchet>();
    tokio::spawn(server);
    let proxy = Proxy::start(server_addr).await?;
    let store = temp_store();
    let (agent, _node) = spawn_agent_with(&store, QUICK).await?;
    let (mut sink, mut stream) = open(agent).await?;
    let name: String = username();
    let cid = register(&mut sink, &mut stream, &proxy.addr.to_string(), &name).await??;

    proxy.strand();
    assert_eq!(
        next_link_event(&mut stream, cid).await?,
        "lost(reconnecting=true)"
    );

    let answer = connect_with_password(&mut sink, &mut stream, &name, "not-the-password").await?;
    assert!(
        matches!(
            answer,
            InternalServiceResponse::ConnectFailure(ref f)
                if f.cid == 0 && f.message == "Invalid username or password"
        ),
        "refused exactly as a wrong password on an untracked account is, naming no session: {answer:?}"
    );

    // Still the reconnect's: when the server lets its dead session go, the reconnect lands.
    proxy.release();
    assert_eq!(
        next_link_event(&mut stream, cid).await?,
        "reconnected",
        "the wrong password stopped or disturbed the reconnect"
    );
    let peers = list_peers(&mut sink, &mut stream, cid).await?;
    assert!(
        matches!(peers, InternalServiceResponse::ListAllPeersResponse(ref r) if r.cid == cid),
        "{peers:?}"
    );
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

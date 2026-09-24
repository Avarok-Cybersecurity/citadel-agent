//! A session whose server drops it without being asked is reconnected under the same CID.
//!
//! A deploy of the workspace server resets its WebSocket, and the agent used to remove
//! every session on that: a deploy signed everyone out. These cut the link with a proxy,
//! as the reset does, and check what the UI hears and what is left afterwards. The
//! decisions themselves (backoff, give-up, refusals) are unit-tested in
//! src/kernel/reconnect/policy_tests.rs.

#[path = "reconnect_support/mod.rs"]
mod reconnect;
// Shared with account_server_host.rs, which uses the helpers this file does not.
#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service_test_common as common;
use citadel_internal_service_types::InternalServiceResponse;
use citadel_sdk::prelude::*;
use reconnect::{connect, disconnect, link_events, list_peers, next_link_event, Proxy};
use std::error::Error;
use std::time::Duration;
use support::{open, register, session, spawn_agent, temp_store, username};

/// Longer than the first two waits and an attempt, so a reconnect that should not
/// happen would have been reported inside it.
const QUIET: Duration = Duration::from_secs(5);

#[tokio::test]
async fn a_dropped_link_comes_back_under_the_same_cid() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let (server, server_addr) = common::server_info_skip_cert_verification::<StackedRatchet>();
    tokio::spawn(server);
    let proxy = Proxy::start(server_addr).await?;
    let store = temp_store();
    let (agent, _node) = spawn_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;
    let cid = register(&mut sink, &mut stream, &proxy.addr.to_string(), &username()).await??;

    proxy.sever();

    assert_eq!(
        next_link_event(&mut stream, cid).await?,
        "lost(reconnecting=true)"
    );
    assert_eq!(next_link_event(&mut stream, cid).await?, "reconnected");
    assert_eq!(session(&mut sink, &mut stream, cid).await?.cid, cid);
    let answer = list_peers(&mut sink, &mut stream, cid).await?;
    assert!(
        matches!(answer, InternalServiceResponse::ListAllPeersResponse(ref r) if r.cid == cid),
        "the server answers over the new link: {answer:?}"
    );

    // And again: the reconnected link is watched like the first one.
    proxy.sever();
    assert_eq!(
        next_link_event(&mut stream, cid).await?,
        "lost(reconnecting=true)"
    );
    assert_eq!(next_link_event(&mut stream, cid).await?, "reconnected");
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

#[tokio::test]
async fn a_user_disconnect_is_not_reconnected() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let (server, server_addr) = common::server_info_skip_cert_verification::<StackedRatchet>();
    tokio::spawn(server);
    let proxy = Proxy::start(server_addr).await?;
    let store = temp_store();
    let (agent, _node) = spawn_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;
    let name = username();
    let cid = register(&mut sink, &mut stream, &proxy.addr.to_string(), &name).await??;

    let answer = disconnect(&mut sink, &mut stream, cid).await?;
    assert!(
        matches!(answer, InternalServiceResponse::DisconnectNotification(_)),
        "{answer:?}"
    );

    assert_eq!(
        link_events(&mut stream, cid, QUIET).await,
        Vec::<String>::new()
    );
    assert!(
        session(&mut sink, &mut stream, cid).await.is_err(),
        "no session is left"
    );
    // Nothing was reconnected behind the map's back either: a fresh login is not refused
    // as already connected.
    let login = connect(&mut sink, &mut stream, &name).await?;
    assert!(
        matches!(login, InternalServiceResponse::ConnectSuccess(ref s) if s.cid == cid),
        "{login:?}"
    );
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

#[tokio::test]
async fn a_user_disconnect_while_reconnecting_ends_it() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let (server, server_addr) = common::server_info_skip_cert_verification::<StackedRatchet>();
    tokio::spawn(server);
    let proxy = Proxy::start(server_addr).await?;
    let store = temp_store();
    let (agent, _node) = spawn_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;
    let name = username();
    let cid = register(&mut sink, &mut stream, &proxy.addr.to_string(), &name).await??;

    proxy.refuse(true);
    proxy.sever();
    assert_eq!(
        next_link_event(&mut stream, cid).await?,
        "lost(reconnecting=true)"
    );
    // Past the first two waits (0.5s, 1s), so the sign-out meets a reconnect that has
    // already been refused, not one that has not started.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let answer = disconnect(&mut sink, &mut stream, cid).await?;
    assert!(
        matches!(answer, InternalServiceResponse::DisconnectNotification(_)),
        "signing out of a session that is reconnecting succeeds: {answer:?}"
    );
    proxy.refuse(false);

    assert_eq!(
        link_events(&mut stream, cid, QUIET).await,
        Vec::<String>::new()
    );
    assert!(
        session(&mut sink, &mut stream, cid).await.is_err(),
        "no session is left"
    );
    let login = connect(&mut sink, &mut stream, &name).await?;
    assert!(
        matches!(login, InternalServiceResponse::ConnectSuccess(ref s) if s.cid == cid),
        "{login:?}"
    );
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

#[tokio::test]
async fn a_login_while_reconnecting_waits_for_the_same_session() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let (server, server_addr) = common::server_info_skip_cert_verification::<StackedRatchet>();
    tokio::spawn(server);
    let proxy = Proxy::start(server_addr).await?;
    let store = temp_store();
    let (agent, _node) = spawn_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;
    let name = username();
    let cid = register(&mut sink, &mut stream, &proxy.addr.to_string(), &name).await??;

    proxy.refuse(true);
    proxy.sever();
    assert_eq!(
        next_link_event(&mut stream, cid).await?,
        "lost(reconnecting=true)"
    );

    // A tab reloading mid-reconnect logs in again. That must not start a second SDK
    // connect for the account: it is told the session exists, and nothing is replaced.
    let login = connect(&mut sink, &mut stream, &name).await?;
    assert!(
        matches!(login, InternalServiceResponse::SessionAlreadyActive(ref s) if s.cid == cid),
        "{login:?}"
    );

    proxy.refuse(false);
    assert_eq!(next_link_event(&mut stream, cid).await?, "reconnected");
    assert_eq!(session(&mut sink, &mut stream, cid).await?.cid, cid);
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

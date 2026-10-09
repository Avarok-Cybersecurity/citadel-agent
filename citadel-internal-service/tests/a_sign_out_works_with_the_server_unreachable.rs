//! Signing out must work when the server cannot be told.
//!
//! The SDK's disconnect waits for the server. With the path to it dead and silent it times out,
//! and the handler used to report a failed sign-out while the protocol session survived, with its
//! map entry already gone: wedged until the agent restarted. The session is now ended here and
//! the answer says the server was not told.

#[allow(dead_code)]
#[path = "reconnect_support/mod.rs"]
mod reconnect;
#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service_test_common as common;
use citadel_internal_service_types::InternalServiceResponse;
use citadel_sdk::prelude::*;
use reconnect::{connect, disconnect, link_events, Proxy};
use std::error::Error;
use std::time::Duration;
use support::{open, register, session, spawn_agent, temp_store, username};

/// Longer than the agent's disconnect budget (10 s) and its abandon, so a hang is reported.
const ANSWER_BUDGET: Duration = Duration::from_secs(40);
/// Long enough for a reconnect the sign-out should have prevented to be reported.
const QUIET: Duration = Duration::from_secs(5);

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_out_with_a_dead_path_ends_the_session_here_and_says_so(
) -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let (server, server_addr) = common::server_info_skip_cert_verification::<StackedRatchet>();
    tokio::spawn(server);
    let proxy = Proxy::start(server_addr).await?;
    let store = temp_store();
    let (agent, _node) = spawn_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;
    let name = username();
    let cid = register(&mut sink, &mut stream, &proxy.addr.to_string(), &name).await??;

    // Both ends held open and silent: the server never acknowledges, nothing is reset.
    proxy.stall();

    let answer = tokio::time::timeout(ANSWER_BUDGET, disconnect(&mut sink, &mut stream, cid))
        .await
        .map_err(|_| "the sign-out was never answered")??;
    match answer {
        InternalServiceResponse::DisconnectNotification(n) => {
            assert_eq!(n.cid, cid);
            assert!(
                n.ended_locally.is_some(),
                "the answer must say the server was not told: {n:?}"
            );
        }
        other => panic!("signing out with the server unreachable must succeed: {other:?}"),
    }

    assert_eq!(
        link_events(&mut stream, cid, QUIET).await,
        Vec::<String>::new(),
        "an ended session is not reconnected"
    );
    assert!(
        session(&mut sink, &mut stream, cid).await.is_err(),
        "no session is left in the agent"
    );
    // The SDK holds none either: a fresh sign-in is not refused as already existing.
    proxy.release();
    let login = connect(&mut sink, &mut stream, &name).await?;
    assert!(
        matches!(login, InternalServiceResponse::ConnectSuccess(ref s) if s.cid == cid),
        "{login:?}"
    );
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

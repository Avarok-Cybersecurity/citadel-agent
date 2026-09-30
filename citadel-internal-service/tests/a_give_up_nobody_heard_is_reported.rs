//! A reconnect that gives up while no UI is attached still tells the next one why.
//!
//! `ServerReconnectFailed` goes to the UI attached at the moment of the give-up. With none
//! attached -- the lid closed through a deploy, the tab closed during the outage -- the
//! session just vanished: measured live on 13400/13401, two sessions given up on while no
//! page was open left the next `GetSessions` answering `sessions: []` and nothing else, and
//! a UI cannot tell "signed out by the server" from "never signed in" from that.
//!
//! The proxy refuses every new link after severing the live one, so the reconnect gives up
//! on the short policy below; the UI that heard the drop has gone by then.

#[allow(dead_code)]
#[path = "reconnect_support/mod.rs"]
mod reconnect;
#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service::{ReconnectPolicy, SERVER_RECONNECT};
use citadel_internal_service_connector::connector::WrappedSink;
use citadel_internal_service_connector::io_interface::tcp::TcpIOInterface;
use citadel_internal_service_test_common as common;
use citadel_internal_service_types::{
    GetSessionsResponse, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::prelude::*;
use reconnect::{connect, next_link_event, Proxy};
use std::error::Error;
use std::time::Duration;
use support::{expect, open, register, spawn_agent_with, temp_store, username, Stream};
use uuid::Uuid;

/// Gives up seconds after the drop, where the agent's waits ten minutes.
const QUICK: ReconnectPolicy = ReconnectPolicy {
    first_delay: Duration::from_millis(500),
    max_delay: Duration::from_secs(1),
    give_up_after: Duration::from_secs(3),
    attempt_timeout: Duration::from_secs(5),
    ..SERVER_RECONNECT
};

#[tokio::test]
async fn a_give_up_with_no_ui_attached_is_reported_to_the_next_one() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let (server, server_addr) = common::server_info_skip_cert_verification::<StackedRatchet>();
    tokio::spawn(server);
    let proxy = Proxy::start(server_addr).await?;
    let store = temp_store();
    let (agent, _node) = spawn_agent_with(&store, QUICK).await?;
    let name = username();
    let cid = {
        let (mut sink, mut stream) = open(agent).await?;
        let cid = register(&mut sink, &mut stream, &proxy.addr.to_string(), &name).await??;
        proxy.refuse(true);
        proxy.sever();
        assert_eq!(
            next_link_event(&mut stream, cid).await?,
            "lost(reconnecting=true)"
        );
        cid
        // The page goes away while the agent is still reconnecting.
    };

    let (mut sink, mut stream) = open(agent).await?;
    let after = until_not_signed_in(&mut sink, &mut stream, cid).await?;
    let reported: Vec<(u64, &str)> = after
        .signed_out
        .iter()
        .map(|s| (s.cid, s.username.as_str()))
        .collect();
    assert_eq!(
        reported,
        vec![(cid, name.as_str())],
        "the give-up is reported to a UI that was not there for it: {after:?}"
    );
    assert!(
        !after.signed_out[0].reason.is_empty(),
        "with the reason the server gave"
    );

    // Signing in again ends it.
    proxy.refuse(false);
    let login = connect(&mut sink, &mut stream, &name).await?;
    assert!(
        matches!(login, InternalServiceResponse::ConnectSuccess(ref s) if s.cid == cid),
        "{login:?}"
    );
    let back = sessions(&mut sink, &mut stream).await?;
    assert!(back.sessions.iter().any(|s| s.cid == cid), "{back:?}");
    assert!(
        back.signed_out.is_empty(),
        "a signed-in account is not reported signed out: {back:?}"
    );
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

async fn sessions(
    sink: &mut WrappedSink<TcpIOInterface>,
    stream: &mut Stream,
) -> Result<GetSessionsResponse, Box<dyn Error>> {
    let request_id = Uuid::new_v4();
    common::send(sink, InternalServiceRequest::GetSessions { request_id }).await?;
    expect(stream, |response| match response {
        InternalServiceResponse::GetSessionsResponse(r) if r.request_id == Some(request_id) => {
            Some(r)
        }
        _ => None,
    })
    .await
}

/// The first `GetSessions` that no longer lists `cid`. This UI hears nothing when the
/// reconnect gives up -- that is the point -- so it asks, as a UI's poll does.
async fn until_not_signed_in(
    sink: &mut WrappedSink<TcpIOInterface>,
    stream: &mut Stream,
    cid: u64,
) -> Result<GetSessionsResponse, Box<dyn Error>> {
    let deadline = tokio::time::Instant::now() + QUICK.give_up_after + support::TIMEOUT;
    let mut poll = tokio::time::interval(Duration::from_millis(250));
    loop {
        poll.tick().await;
        let answer = sessions(sink, stream).await?;
        if !answer.sessions.iter().any(|s| s.cid == cid) {
            return Ok(answer);
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("{cid} was never given up on: {answer:?}").into());
        }
    }
}

//! An app's `citadel.session` end to end (`citadel-session-abi.md` §4, plan §4 H2): one agent
//! holding an identity, a real in-process workspace server the member signs in to, and a real
//! app server behind a front that can cut the link, driven through the UI's request protocol.
//!
//! Controls, each applied, seen red and reverted:
//! - recompute the CID on reconnect: build `rules::app_handle` with the session number in it
//!   (`{id_fp}.app-{install}-{session}`), so a reopen derives a new account → the reopened
//!   session's `server_cid` differs and `an_app_session_reconnects_under_its_cid` goes red.
//! - grant all servers (make `decide_open`'s `granted` always true) → the ungranted open
//!   reaches the agent's session machinery and the same test goes red at "ungranted".

#[allow(dead_code)]
#[path = "app_session_support/mod.rs"]
mod app;
#[allow(dead_code)]
#[path = "identity_support/mod.rs"]
mod identity;
#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use app::{spawn_agent, spawn_behind_front, spawn_direct, APP_ID, ECHO};
use citadel_internal_service_test_common as common;
use citadel_internal_service_types::{
    AppSessionState, IdentityDiscovery, InternalServiceRequest, InternalServiceResponse,
    SessionStatus,
};
use identity::{create_identity, signed_in, Sink, Stream};
use std::error::Error;
use uuid::Uuid;

const INSTALL: &str = "0f3a9c2e-1d2e-4f00-9a00-000000000001";

fn open_request(request_id: Uuid, cid: u64, acting: &str, server: &str) -> InternalServiceRequest {
    InternalServiceRequest::AppSessionOpen {
        request_id,
        cid,
        app_id: APP_ID.to_string(),
        install_id: INSTALL.to_string(),
        acting_username: acting.to_string(),
        server: server.to_string(),
    }
}

/// The answer to `request_id`: the state, or the failure's reason.
async fn answered(
    stream: &mut Stream,
    request_id: Uuid,
) -> Result<Result<AppSessionState, String>, Box<dyn Error>> {
    support::expect(stream, |r| match r {
        InternalServiceResponse::AppSessionState(s) if s.request_id == Some(request_id) => {
            Some(Ok(s))
        }
        InternalServiceResponse::AppSessionFailure(f) if f.request_id == Some(request_id) => {
            Some(Err(f.reason))
        }
        _ => None,
    })
    .await
}

async fn open(
    sink: &mut Sink,
    stream: &mut Stream,
    cid: u64,
    acting: &str,
    server: &str,
) -> Result<Result<AppSessionState, String>, Box<dyn Error>> {
    let request_id = Uuid::new_v4();
    common::send(sink, open_request(request_id, cid, acting, server)).await?;
    answered(stream, request_id).await
}

/// The next state the session's link reports, unasked.
async fn next_state(stream: &mut Stream, session: u32) -> Result<AppSessionState, Box<dyn Error>> {
    support::expect(stream, |r| match r {
        InternalServiceResponse::AppSessionState(s)
            if s.request_id.is_none() && s.session == session =>
        {
            Some(s)
        }
        _ => None,
    })
    .await
}

/// Sends `body` and waits for the server's echo of it.
async fn echoed(
    sink: &mut Sink,
    stream: &mut Stream,
    cid: u64,
    session: u32,
    body: &[u8],
) -> Result<(), Box<dyn Error>> {
    let send = InternalServiceRequest::AppSessionSend {
        request_id: Uuid::new_v4(),
        cid,
        install_id: INSTALL.to_string(),
        session,
        body: body.to_vec(),
    };
    common::send(sink, send).await?;
    let want = [ECHO, body].concat();
    support::expect(stream, |r| match r {
        InternalServiceResponse::AppSessionMessage(m) if m.session == session && m.cid == cid => {
            Some(Ok(m.body))
        }
        InternalServiceResponse::AppSessionFailure(f) => Some(Err(f.reason)),
        _ => None,
    })
    .await?
    .map(|got| assert_eq!(got, want))
    .map_err(Into::into)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_session_reconnects_under_its_cid() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let (app, cut) = spawn_behind_front([31; 32]).await?;
    let member_server = spawn_direct([32; 32], vec![app.domain.clone()]).await?;
    let store = support::temp_store();
    let agent = spawn_agent(&app, &store).await?;
    let (mut sink, mut stream) = support::open(agent).await?;
    create_identity(&mut sink, &mut stream).await?;

    let request_id = Uuid::new_v4();
    let register = InternalServiceRequest::RegisterIdentity {
        resume_lifetime_s: None,
        request_id,
        server_addr: member_server.domain.clone(),
        handle: "alice".to_string(),
        full_name: "App Session Test".to_string(),
        discovery: IdentityDiscovery::Pinned {
            server_fp: member_server.fp(),
        },
        session_security_settings: citadel_sdk::prelude::default_session_security_settings(),
        server_password: None,
        admission_token: None,
    };
    common::send(&mut sink, register).await?;
    let cid = signed_in(&mut stream, request_id).await??.cid;
    let acting = format!("alice@{}", member_server.domain);

    // Ungranted: refused, and the app server is never dialled.
    let refused = open(&mut sink, &mut stream, cid, &acting, "localhost:9").await?;
    assert_eq!(refused.err().as_deref(), Some("ungranted"));
    assert!(app.accepted.lock().is_empty(), "the agent dialled nothing");
    let stranger = open(
        &mut sink,
        &mut stream,
        cid,
        "mallory@elsewhere",
        &app.domain,
    )
    .await?;
    assert_eq!(
        stranger.err().as_deref(),
        Some("session"),
        "acting as another user"
    );

    let opened = open(&mut sink, &mut stream, cid, &acting, &app.domain).await??;
    assert_eq!(opened.state, SessionStatus::Connected);
    let (session, server_cid) = (opened.session, opened.server_cid);
    assert_ne!(server_cid, 0);
    assert_ne!(server_cid, cid, "the app's account is not the member's");
    echoed(&mut sink, &mut stream, cid, session, b"ping").await?;
    assert!(
        support::session(&mut sink, &mut stream, server_cid)
            .await
            .is_err(),
        "the agent's app account is not listed as a member session"
    );

    // Forced close: every link to the app server is cut.
    cut.send_modify(|n| *n += 1);
    assert_eq!(
        next_state(&mut stream, session).await?.state,
        SessionStatus::Connecting
    );
    let back = next_state(&mut stream, session).await?;
    assert_eq!(
        (back.state, back.server_cid),
        (SessionStatus::Connected, server_cid)
    );
    echoed(&mut sink, &mut stream, cid, session, b"again").await?;

    // Closed by the app and opened again: the same account, never a second one.
    let request_id = Uuid::new_v4();
    let close = InternalServiceRequest::AppSessionClose {
        request_id,
        cid,
        install_id: INSTALL.to_string(),
        session,
    };
    common::send(&mut sink, close).await?;
    let closed = answered(&mut stream, request_id).await??;
    assert!(
        matches!(closed.state, SessionStatus::Closed { .. }),
        "{closed:?}"
    );
    let reopened = open(&mut sink, &mut stream, cid, &acting, &app.domain).await??;
    assert_eq!(
        reopened.server_cid, server_cid,
        "reopened under the same CID"
    );
    echoed(&mut sink, &mut stream, cid, reopened.session, b"third").await?;

    let accepted = app.accepted.lock().clone();
    assert!(accepted.len() >= 3, "{accepted:?}");
    assert!(
        accepted.iter().all(|c| *c == server_cid),
        "the server only ever saw one CID: {accepted:?}"
    );
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

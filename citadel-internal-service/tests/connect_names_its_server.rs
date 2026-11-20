//! A Connect names the server, and the agent signs in only the account with that username
//! on that server.
//!
//! The SDK finds an account by username alone and dials whatever server it was registered
//! to, so a sign-in meant for one workspace reached another. Here alice is registered on
//! server A; a Connect naming server B must be refused, typed, before anything is dialled --
//! with the old resolution it signed in to A.
//!
//! Two accounts called alice on two servers cannot both exist in one agent: the SDK's CID is
//! `username_to_cid(username)` (citadel_types user/mod.rs) and the client stores accounts by
//! CID, so this test holds one alice and names the other server.

#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service_test_common as common;
use citadel_internal_service_types::{
    FailureReason, InternalServiceRequest, InternalServiceResponse,
};
use std::error::Error;
use support::{expect, open, register, spawn_agent, spawn_server, temp_store, username, PASSWORD};
use uuid::Uuid;

type Sink = citadel_internal_service_connector::connector::WrappedSink<
    citadel_internal_service_connector::io_interface::tcp::TcpIOInterface,
>;

/// Connect `name` naming `server`: Ok(cid) on success, Err((reason, message)) on refusal.
async fn sign_in(
    sink: &mut Sink,
    stream: &mut support::Stream,
    name: &str,
    server: &str,
) -> Result<Result<u64, (Option<FailureReason>, String)>, Box<dyn Error>> {
    let request_id = Uuid::new_v4();
    common::send(
        sink,
        InternalServiceRequest::Connect {
            request_id,
            username: name.to_string(),
            server_address: server.to_string(),
            password: Some(PASSWORD.as_bytes().to_vec().into()),
            security_key: false,
            recovery_phrase: None,
            connect_mode: citadel_sdk::prelude::ConnectMode::Standard { force_login: false },
            udp_mode: Default::default(),
            keep_alive_timeout: None,
            session_security_settings: citadel_sdk::prelude::default_session_security_settings(),
            server_password: None,
            admission_token: None,
        },
    )
    .await?;
    expect(stream, |response| match response {
        InternalServiceResponse::ConnectSuccess(s) if s.request_id == Some(request_id) => {
            Some(Ok(s.cid))
        }
        InternalServiceResponse::ConnectFailure(f) if f.request_id == Some(request_id) => {
            Some(Err((f.reason_code, f.message)))
        }
        _ => None,
    })
    .await
}

#[tokio::test]
async fn a_connect_signs_in_only_the_account_on_the_server_it_names() -> Result<(), Box<dyn Error>>
{
    common::setup_log();
    let (port_a, port_b) = (spawn_server().await?, spawn_server().await?);
    let (server_a, server_b) = (format!("localhost:{port_a}"), format!("localhost:{port_b}"));
    let store = temp_store();
    let (agent, _node) = spawn_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;
    let name = username();

    // Registered on A, and signed in there by the registration's own Connect.
    let cid = register(&mut sink, &mut stream, &server_a, &name).await??;
    let request_id = Uuid::new_v4();
    common::send(
        &mut sink,
        InternalServiceRequest::Disconnect { request_id, cid },
    )
    .await?;
    expect(&mut stream, |r| {
        matches!(r, InternalServiceResponse::DisconnectNotification(_)).then_some(())
    })
    .await?;

    // Naming B: refused, typed, and nothing signed in.
    let wrong = sign_in(&mut sink, &mut stream, &name, &server_b).await?;
    assert!(
        matches!(wrong, Err((Some(FailureReason::AccountOnAnotherServer), _))),
        "a Connect naming B signed in to A's account: {wrong:?}"
    );

    // A username this agent holds no account for: typed too.
    let nobody = sign_in(&mut sink, &mut stream, &format!("{name}.nobody"), &server_a).await?;
    assert!(
        matches!(nobody, Err((Some(FailureReason::NoSuchAccount), _))),
        "{nobody:?}"
    );

    // Naming none (a page built before the field): answered, untyped so that page can read
    // it, telling it to reload -- not dropped to a 30-second timeout.
    let outdated = sign_in(&mut sink, &mut stream, &name, "").await?;
    assert!(
        matches!(&outdated, Err((None, message)) if message.contains("Reload the page")),
        "{outdated:?}"
    );

    // Naming A, however it is written: the account, signed in.
    let right = sign_in(&mut sink, &mut stream, &name, &server_a).await?;
    assert_eq!(right.map_err(|e| format!("{e:?}"))?, cid);
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

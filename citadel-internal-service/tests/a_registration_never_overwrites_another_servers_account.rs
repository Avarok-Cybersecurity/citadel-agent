//! A Register never overwrites this agent's account of the same username on another server.
//!
//! One agent holds one account per username: the SDK keys a stored account by
//! `username_to_cid(username)` and saves a new registration over whatever is there with no
//! check (citadel_user account_manager `register_personal_hyperlan_server`). Registering
//! alice on B while the agent held alice on A erased A's account here, silently. The agent
//! now refuses that Register, typed, before anything is dialled -- and A still signs in.

#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service_test_common as common;
use citadel_internal_service_types::{
    FailureReason, InternalServiceRequest, InternalServiceResponse, RegisterFailure,
};
use std::error::Error;
use support::{
    account, expect, open, register, spawn_agent, spawn_server, temp_store, username, PASSWORD,
};
use uuid::Uuid;

#[tokio::test]
async fn a_name_held_for_another_server_is_refused_and_that_account_survives(
) -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let (port_a, port_b) = (spawn_server().await?, spawn_server().await?);
    let (server_a, server_b) = (format!("localhost:{port_a}"), format!("localhost:{port_b}"));
    let store = temp_store();
    let (agent, _node) = spawn_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;
    let name = username();

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

    // The same name on B: refused, typed, naming A.
    let request_id = Uuid::new_v4();
    common::send(
        &mut sink,
        InternalServiceRequest::Register {
            request_id,
            server_addr: server_b.clone(),
            full_name: "Server Host".to_string(),
            username: name.clone(),
            proposed_password: PASSWORD.as_bytes().to_vec().into(),
            connect_after_register: true,
            session_security_settings: Default::default(),
            server_password: None,
            admission_token: None,
        },
    )
    .await?;
    let answer = expect(&mut stream, |response| match response {
        InternalServiceResponse::RegisterFailure(f) if f.request_id == Some(request_id) => {
            Some(Err(f))
        }
        InternalServiceResponse::ConnectSuccess(s) if s.request_id == Some(request_id) => {
            Some(Ok(s.cid))
        }
        _ => None,
    })
    .await?;
    let refused: RegisterFailure = match answer {
        Err(failure) => failure,
        Ok(new_cid) => {
            panic!("registering {name} on B succeeded ({new_cid}) and overwrote A's account")
        }
    };
    assert_eq!(
        refused.reason_code,
        Some(FailureReason::UsernameHeldForAnotherServer),
        "{refused:?}"
    );
    assert_eq!(refused.held_for_server.as_deref(), Some(server_a.as_str()));

    // A's account is intact: still on A, and it signs in there.
    let accounts = account(&mut sink, &mut stream, None).await?;
    let held = accounts
        .iter()
        .find(|(_, a)| a.username == name)
        .ok_or("A's account is gone")?;
    assert_eq!(held.0, cid);
    assert_eq!(held.1.server_host.as_deref(), Some(server_a.as_str()));
    let request_id = Uuid::new_v4();
    common::send(
        &mut sink,
        InternalServiceRequest::Connect {
            request_id,
            username: name.clone(),
            server_address: server_a.clone(),
            password: Some(PASSWORD.as_bytes().to_vec().into()),
            security_key: false,
            recovery_code: None,
            recovery_phrase: None,
            connect_mode: citadel_sdk::prelude::ConnectMode::Standard { force_login: false },
            udp_mode: Default::default(),
            keep_alive_timeout: None,
            session_security_settings: Default::default(),
            server_password: None,
            admission_token: None,
        },
    )
    .await?;
    let signed_in = expect(&mut stream, |response| match response {
        InternalServiceResponse::ConnectSuccess(s) if s.request_id == Some(request_id) => {
            Some(Ok(s.cid))
        }
        InternalServiceResponse::ConnectFailure(f) if f.request_id == Some(request_id) => {
            Some(Err(f.message))
        }
        _ => None,
    })
    .await?;
    assert_eq!(signed_in?, cid);
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

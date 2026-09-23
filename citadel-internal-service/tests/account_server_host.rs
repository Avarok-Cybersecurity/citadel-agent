//! An account reports the server host the user typed, not only the address it resolved to.
//!
//! Behind an edge (`acme.work.avarok.net` -> an anycast address) the resolved address is
//! shared by every workspace, so `GetAccountInformation` and `GetSessions` carry the typed
//! `host[:port]` as `server_host`, recorded at registration and kept across agent restarts.

#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service_test_common as common;
use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
use citadel_sdk::prelude::*;
use std::error::Error;
use std::time::Duration;
use support::{
    account, expect, open, register, session, spawn_agent, spawn_server, temp_store, username,
    PASSWORD,
};
use uuid::Uuid;

#[tokio::test]
async fn a_hostname_registration_reports_the_typed_host() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let port = spawn_server().await?;
    let store = temp_store();
    let (agent, _node) = spawn_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;

    let typed = format!("localhost:{port}");
    let cid = register(&mut sink, &mut stream, &typed, &username()).await??;

    let accounts = account(&mut sink, &mut stream, Some(cid)).await?;
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].1.server_host.as_deref(), Some(typed.as_str()));
    let session = session(&mut sink, &mut stream, cid).await?;
    assert_eq!(session.server_host.as_deref(), Some(typed.as_str()));
    assert_ne!(
        session.server_address, typed,
        "server_address is still the resolved one"
    );
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

#[tokio::test]
async fn an_ip_registration_reports_the_ip_string() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let (server, server_addr) = common::server_info_skip_cert_verification::<StackedRatchet>();
    tokio::spawn(server);
    let store = temp_store();
    let (agent, _node) = spawn_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;

    let typed = server_addr.to_string();
    let cid = register(&mut sink, &mut stream, &typed, &username()).await??;

    let accounts = account(&mut sink, &mut stream, Some(cid)).await?;
    assert_eq!(accounts[0].1.server_host.as_deref(), Some(typed.as_str()));
    let session = session(&mut sink, &mut stream, cid).await?;
    assert_eq!(session.server_host.as_deref(), Some(typed.as_str()));
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

#[tokio::test]
async fn the_host_survives_an_agent_restart() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let port = spawn_server().await?;
    let store = temp_store();
    let typed = format!("localhost:{port}");
    let name = username();

    let (agent, node) = spawn_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;
    let first_cid = register(&mut sink, &mut stream, &typed, &name).await??;
    // Log out first: an aborted node's SDK tasks outlive it, so the server would
    // keep the old session and refuse the login below as already connected.
    common::send(
        &mut sink,
        InternalServiceRequest::Disconnect {
            request_id: Uuid::new_v4(),
            cid: first_cid,
        },
    )
    .await?;
    expect(&mut stream, |response| match response {
        InternalServiceResponse::DisconnectNotification(_) => Some(()),
        _ => None,
    })
    .await?;
    drop((sink, stream));
    node.abort();
    let _ = node.await;

    let (agent, _node) = spawn_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;
    let accounts = account(&mut sink, &mut stream, None).await?;
    assert_eq!(accounts.len(), 1, "the restarted agent loaded the account");
    let (cid, info) = &accounts[0];
    assert_eq!(info.username, name);
    assert_eq!(info.server_host.as_deref(), Some(typed.as_str()));

    // A login after the restart reports it too. The server may still hold the
    // aborted agent's session for a moment, so retry until it lets go.
    let mut connected = None;
    for _ in 0..20 {
        common::send(
            &mut sink,
            InternalServiceRequest::Connect {
                request_id: Uuid::new_v4(),
                username: name.clone(),
                password: PASSWORD.as_bytes().to_vec().into(),
                connect_mode: Default::default(),
                udp_mode: Default::default(),
                keep_alive_timeout: None,
                session_security_settings: Default::default(),
                server_password: None,
            },
        )
        .await?;
        let outcome = expect(&mut stream, |response| match response {
            InternalServiceResponse::ConnectSuccess(success) => Some(Ok(success.cid)),
            InternalServiceResponse::ConnectFailure(failure) => Some(Err(failure.message)),
            _ => None,
        })
        .await?;
        match outcome {
            Ok(cid) => {
                connected = Some(cid);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
    let connected = connected.ok_or("never reconnected after the restart")?;
    assert_eq!(connected, *cid);
    let session = session(&mut sink, &mut stream, connected).await?;
    assert_eq!(session.server_host.as_deref(), Some(typed.as_str()));
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

#[tokio::test]
async fn an_invalid_server_host_is_refused_and_nothing_is_recorded() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let port = spawn_server().await?;
    let store = temp_store();
    let (agent, _node) = spawn_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;

    for typed in [
        format!("localhost:{port}/workspace"),
        format!("user@localhost:{port}"),
        format!("{}.localhost:{port}", "a".repeat(260)),
    ] {
        let refused = register(&mut sink, &mut stream, &typed, &username()).await?;
        let message = refused.expect_err(&format!("{typed:?} should be refused"));
        assert!(
            message.contains("invalid server address"),
            "{typed:?}: {message}"
        );
    }
    assert!(account(&mut sink, &mut stream, None).await?.is_empty());
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

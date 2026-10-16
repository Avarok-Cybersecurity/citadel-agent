//! An account stored on this machine reports the highest security level it can sign in at.
//!
//! The SDK refuses a Connect above the level the account registered with
//! (`SessionSecurityExceedsRegistration`). The login form asked for Extreme on a Standard
//! account and the refusal reached the user as "Network error"; with the level in
//! `GetAccountInformation`, the form offers only the levels the account can use.

#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service_test_common as common;
use citadel_sdk::prelude::*;
use std::error::Error;
use support::{account, open, register_with, spawn_agent, temp_store, username};

async fn reported_level(registered: SecurityLevel) -> Result<Option<u8>, Box<dyn Error>> {
    let (server, server_addr) = common::server_info_skip_cert_verification::<StackedRatchet>();
    tokio::spawn(server);
    let store = temp_store();
    let (agent, _node) = spawn_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;
    let settings = SessionSecuritySettingsBuilder::default()
        .with_security_level(registered)
        .build()?;
    let cid = register_with(
        &mut sink,
        &mut stream,
        &server_addr.to_string(),
        &username(),
        settings,
    )
    .await??;
    let accounts = account(&mut sink, &mut stream, Some(cid)).await?;
    let _ = std::fs::remove_dir_all(&store);
    // SecurityLevel has no PartialEq; its byte value is the SDK's own ordering.
    Ok(accounts[0]
        .1
        .registered_security_level
        .map(SecurityLevel::value))
}

#[tokio::test]
async fn a_standard_account_reports_standard() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    assert_eq!(
        reported_level(SecurityLevel::Standard).await?,
        Some(SecurityLevel::Standard.value())
    );
    Ok(())
}

#[tokio::test]
async fn an_extreme_account_reports_extreme() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    assert_eq!(
        reported_level(SecurityLevel::Extreme).await?,
        Some(SecurityLevel::Extreme.value())
    );
    Ok(())
}

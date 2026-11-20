//! Account recovery through the agent: the phrase a registration issued recovers the account on
//! a second device (the first is revoked), signs in to the recovery console there, and the
//! console sets a new password with no step-up. A full session's password change needs the
//! current password; a regenerated phrase replaces the old one at once.

use citadel_internal_service_test_common::pq::PASSWORD;
use citadel_internal_service_test_common::pq::{spawn_agent, spawn_server, username, Server};
use citadel_internal_service_test_common::pq_accounts::password_offer;
use citadel_internal_service_test_common::pq_window::{Offer, Window};
use citadel_internal_service_test_common::setup_log;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, SignInManagementOp, SignInManagementOutcome,
};
use citadel_sdk::prelude::{RecoverySeed, SecBuffer};
use std::error::Error;
use std::net::SocketAddr;
use uuid::Uuid;

const NEW_PASSWORD: &str = "a fresh start, remembered";

fn password(password: &str) -> Offer {
    Offer {
        password: Some(password.to_string()),
        ..Default::default()
    }
}

fn set_password(new_password: &str) -> SignInManagementOp {
    SignInManagementOp::SetPassword {
        new_password: SecBuffer::from(new_password),
    }
}

/// A full session can list its peers; a recovery console cannot.
async fn is_full_session(window: &mut Window, cid: u64) -> Result<bool, Box<dyn Error>> {
    let request_id = Uuid::new_v4();
    window
        .send(InternalServiceRequest::ListRegisteredPeers { request_id, cid })
        .await?;
    let answer = window.answer_of(request_id, None).await?.response;
    Ok(matches!(
        answer,
        InternalServiceResponse::ListRegisteredPeersResponse(_)
    ))
}

/// A registered, signed-out account and its phrase as typed.
async fn registered(
    window: &mut Window,
    server: SocketAddr,
) -> Result<(String, u64, String), Box<dyn Error>> {
    let user = username("rec");
    let registered = window.register(server, &user, PASSWORD).await??;
    Ok((user, registered.cid, registered.typed_phrase()))
}

#[tokio::test]
async fn the_phrase_recovers_the_account_on_a_second_device_and_the_console_sets_a_password(
) -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_server(Server::PostQuantum);
    let mut first = Window::open(spawn_agent().await?).await?;
    let (user, cid, typed) = registered(&mut first, server).await?;

    let mut second = Window::open(spawn_agent().await?).await?;
    let recovered = second
        .recover(server, &user, &typed, "Second laptop")
        .await?;
    assert_eq!(
        recovered.recovered_cid,
        Some(cid),
        "the second device was not told it recovered the account"
    );
    let console = recovered.sign_in?;
    assert_eq!(console, cid, "the phrase signed in to another account");
    assert!(
        !is_full_session(&mut second, console).await?,
        "the phrase opened a full session"
    );

    // The console proves nothing again: no step-up.
    let set = second.manage(console, set_password(NEW_PASSWORD), None, None);
    assert_eq!(set.await??, SignInManagementOutcome::PasswordSet);
    second.disconnect(console).await?;

    let signed_in = second
        .sign_in(server, &user, &password(NEW_PASSWORD))
        .await??;
    assert!(
        is_full_session(&mut second, signed_in).await?,
        "the new password opened only a console"
    );
    second.disconnect(signed_in).await?;
    let old = second.sign_in(server, &user, &password(PASSWORD)).await?;
    assert!(old.is_err(), "the forgotten password still signs in");

    // The first device's bundle was revoked with the recovery.
    let revoked = first
        .sign_in(server, &user, &password(NEW_PASSWORD))
        .await?;
    assert!(revoked.is_err(), "the revoked device still signs in");
    Ok(())
}

#[tokio::test]
async fn on_the_device_that_holds_the_account_the_phrase_only_signs_in(
) -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_server(Server::PostQuantum);
    let mut window = Window::open(spawn_agent().await?).await?;
    let (user, cid, typed) = registered(&mut window, server).await?;

    let recovered = window.recover(server, &user, &typed, "This laptop").await?;
    assert_eq!(
        recovered.recovered_cid, None,
        "a device that holds the account recovered it (and revoked others)"
    );
    assert_eq!(recovered.sign_in?, cid);
    Ok(())
}

#[tokio::test]
async fn a_wrong_phrase_recovers_nothing_and_a_mistyped_one_is_refused_before_dialling(
) -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_server(Server::PostQuantum);
    let mut first = Window::open(spawn_agent().await?).await?;
    let (user, _, _) = registered(&mut first, server).await?;
    let mut second = Window::open(spawn_agent().await?).await?;

    let another = RecoverySeed::from_bytes([42; 32]).words().join(" ");
    let wrong = second.recover(server, &user, &another, "Laptop").await?;
    assert!(
        wrong.recovered_cid.is_none() && wrong.sign_in.is_err(),
        "{wrong:?}"
    );

    let mistyped = second
        .recover(server, &user, "abandon ability able", "Laptop")
        .await?;
    let message = mistyped.sign_in.expect_err("a three-word phrase recovered");
    assert!(message.contains("24 words"), "{message}");

    // The account was not touched: its own device still signs in with its password.
    let still = first.sign_in(server, &user, &password_offer()).await?;
    assert!(
        still.is_ok(),
        "a refused recovery revoked the device: {still:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_full_session_changes_its_password_only_with_the_current_one(
) -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_server(Server::PostQuantum);
    let mut window = Window::open(spawn_agent().await?).await?;
    let (user, _, _) = registered(&mut window, server).await?;
    let cid = window.sign_in(server, &user, &password_offer()).await??;

    let no_step_up = window
        .manage(cid, set_password(NEW_PASSWORD), None, None)
        .await?;
    assert!(
        no_step_up.is_err(),
        "a full session set a password unproven"
    );
    let wrong = window
        .manage(cid, set_password(NEW_PASSWORD), Some("not it"), None)
        .await?;
    assert!(wrong.is_err(), "a wrong current password changed it");
    let set = window
        .manage(cid, set_password(NEW_PASSWORD), Some(PASSWORD), None)
        .await??;
    assert_eq!(set, SignInManagementOutcome::PasswordSet);
    window.disconnect(cid).await?;

    let signed_in = window
        .sign_in(server, &user, &password(NEW_PASSWORD))
        .await?;
    assert!(signed_in.is_ok(), "the new password: {signed_in:?}");
    Ok(())
}

#[tokio::test]
async fn a_regenerated_phrase_replaces_the_old_one_at_once() -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_server(Server::PostQuantum);
    let mut window = Window::open(spawn_agent().await?).await?;
    let (user, _, old) = registered(&mut window, server).await?;
    let console = Offer {
        recovery_phrase: Some(old.clone()),
        ..Default::default()
    };
    let cid = window.sign_in(server, &user, &console).await??;

    let SignInManagementOutcome::RecoveryPhrase(new) = window
        .manage(
            cid,
            SignInManagementOp::RegenerateRecoveryPhrase,
            None,
            None,
        )
        .await??
    else {
        panic!("no new phrase");
    };
    let new = new.0.join(" ");
    assert_ne!(new, old);
    window.disconnect(cid).await?;

    let with_old = window.sign_in(server, &user, &console).await?;
    assert!(with_old.is_err(), "the replaced phrase still signs in");
    let with_new = Offer {
        recovery_phrase: Some(new),
        ..Default::default()
    };
    let signed_in = window.sign_in(server, &user, &with_new).await?;
    assert!(signed_in.is_ok(), "the new phrase: {signed_in:?}");
    Ok(())
}

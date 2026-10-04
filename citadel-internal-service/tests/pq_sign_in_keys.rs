//! Security keys through the agent: the SDK's key request reaches the signing-in window as a
//! `SecurityKeyChallengeNotification`, the window's answer goes back to the SDK, and the server
//! decides. The window here is a fake PRF responder: it answers every challenge with a fixed
//! PRF output for one credential, as a browser relaying WebAuthn would.

use citadel_internal_service_test_common::pq::{spawn_agent, spawn_server, Server, CRED};
use citadel_internal_service_test_common::pq_accounts::{account, password_offer};
use citadel_internal_service_test_common::pq_window::{FakeKey, Offer, Window};
use citadel_internal_service_test_common::setup_log;
use citadel_internal_service_types::SignInPolicy;
use citadel_internal_service_types::{InternalServiceResponse, SecurityKeyPurpose};
use std::error::Error;

fn password_and(key: &FakeKey) -> Offer {
    Offer {
        key: Some(key.clone()),
        ..password_offer()
    }
}

#[tokio::test]
async fn a_password_and_key_account_signs_in_with_both_and_not_with_the_password_alone(
) -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_server(Server::PostQuantum);
    let mut window = Window::open(spawn_agent().await?).await?;
    let key = FakeKey::new(7);
    let user = account(&mut window, server, SignInPolicy::PasswordAndKey, &key).await?;

    let alone = window.sign_in(&user.username, &password_offer()).await?;
    assert!(alone.is_err(), "the password alone signed in: {alone:?}");

    let both = window.connect(&user.username, &password_and(&key)).await?;
    assert!(
        matches!(both.response, InternalServiceResponse::ConnectSuccess(_)),
        "password and key did not sign in: {:?}",
        both.response
    );
    let [challenge] = both.challenges.as_slice() else {
        panic!("expected one key challenge, got {:?}", both.challenges);
    };
    assert_eq!(challenge.purpose, SecurityKeyPurpose::SignIn);
    assert_eq!(challenge.cid, 0, "a sign-in has no session yet");
    assert_eq!(challenge.allowed_credential_ids, vec![CRED.to_vec()]);
    assert_eq!(challenge.prf_salt.len(), 32);
    Ok(())
}

#[tokio::test]
async fn a_wrong_prf_output_is_refused() -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_server(Server::PostQuantum);
    let mut window = Window::open(spawn_agent().await?).await?;
    let user = account(
        &mut window,
        server,
        SignInPolicy::PasswordAndKey,
        &FakeKey::new(7),
    )
    .await?;

    let another_key = FakeKey::new(8);
    let wrong = window
        .connect(&user.username, &password_and(&another_key))
        .await?;
    assert_eq!(wrong.challenges.len(), 1, "the key was not asked for");
    assert!(
        matches!(wrong.response, InternalServiceResponse::ConnectFailure(_)),
        "another key's PRF signed in: {:?}",
        wrong.response
    );
    Ok(())
}

#[tokio::test]
async fn a_key_only_account_signs_in_with_no_password() -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_server(Server::PostQuantum);
    let mut window = Window::open(spawn_agent().await?).await?;
    let key = FakeKey::new(9);
    let user = account(&mut window, server, SignInPolicy::KeyOnly, &key).await?;

    let key_only = Offer {
        key: Some(key),
        ..Default::default()
    };
    let signed_in = window.sign_in(&user.username, &key_only).await?;
    assert!(
        signed_in.is_ok(),
        "the key alone did not sign in: {signed_in:?}"
    );
    let no_key = window.sign_in(&user.username, &password_offer()).await?;
    assert!(
        no_key.is_err(),
        "a key-only account signed in on its password"
    );
    Ok(())
}

/// A session a key opened records no password fingerprint: a second window that knows only
/// the password must not be handed it (and could not attach to it on that password either).
#[tokio::test]
async fn a_key_gated_session_is_not_handed_to_its_password_alone() -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_server(Server::PostQuantum);
    let agent = spawn_agent().await?;
    let mut first = Window::open(agent).await?;
    let key = FakeKey::new(7);
    let user = account(&mut first, server, SignInPolicy::PasswordAndKey, &key).await?;
    first.sign_in(&user.username, &password_and(&key)).await??;

    let mut second = Window::open(agent).await?;
    let reused = second.connect(&user.username, &password_offer()).await?;
    assert!(
        matches!(reused.response, InternalServiceResponse::ConnectFailure(ref f) if f.cid == 0),
        "the password alone was handed a key-gated session: {:?}",
        reused.response
    );
    assert!(reused.challenges.is_empty());
    Ok(())
}

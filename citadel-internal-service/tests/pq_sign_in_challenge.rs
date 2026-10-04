//! The key-challenge relay's own rules: only the windows a challenge was sent to may answer it,
//! a bad answer does not spend it, only the first valid answer counts, a decline fails the
//! sign-in at once, and an unanswered challenge fails at the SDK's deadline and no later.

use citadel_internal_service_test_common::pq::{spawn_agent, spawn_server, Server, PASSWORD};
use citadel_internal_service_test_common::pq_accounts::account;
use citadel_internal_service_test_common::pq_window::{FakeKey, Window};
use citadel_internal_service_test_common::setup_log;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, SecurityKeyChallengeNotification,
};
use citadel_sdk::prelude::{ConnectMode, SecBuffer, SignInPolicy, UdpMode};
use std::error::Error;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// The SDK's `KEY_PRESENCE_WINDOW`.
const KEY_WINDOW: Duration = Duration::from_secs(60);

/// Starts a password-and-key sign-in and returns its request id and the challenge it raised.
async fn challenged(
    window: &mut Window,
    username: &str,
) -> Result<(Uuid, SecurityKeyChallengeNotification), Box<dyn Error>> {
    let request_id = Uuid::new_v4();
    window
        .send(InternalServiceRequest::Connect {
            request_id,
            username: username.to_string(),
            password: Some(SecBuffer::from(PASSWORD)),
            security_key: true,
            recovery_code: None,
            connect_mode: ConnectMode::Standard { force_login: false },
            udp_mode: UdpMode::Disabled,
            keep_alive_timeout: None,
            session_security_settings: Default::default(),
            server_password: None,
            admission_token: None,
        })
        .await?;
    match window.next_of(request_id).await? {
        InternalServiceResponse::SecurityKeyChallengeNotification(challenge) => {
            Ok((request_id, challenge))
        }
        other => Err(format!("no key challenge: {other:?}").into()),
    }
}

/// Sends an answer under its own request id and returns the agent's verdict on it.
async fn answer(
    window: &mut Window,
    challenge: &SecurityKeyChallengeNotification,
    credential_id: &[u8],
    prf: &[u8],
) -> Result<Result<(), String>, Box<dyn Error>> {
    let request_id = Uuid::new_v4();
    window
        .send(InternalServiceRequest::SecurityKeyAnswer {
            request_id,
            challenge_id: challenge.challenge_id,
            credential_id: credential_id.to_vec(),
            prf_output: SecBuffer::from(prf.to_vec()),
        })
        .await?;
    Ok(match window.next_of(request_id).await? {
        InternalServiceResponse::SecurityKeyAnswerSuccess(_) => Ok(()),
        InternalServiceResponse::SecurityKeyAnswerFailure(refused) => Err(refused.message),
        other => Err(format!("{other:?}")),
    })
}

#[tokio::test]
async fn only_the_first_valid_answer_from_an_asked_window_counts() -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_server(Server::PostQuantum);
    let agent = spawn_agent().await?;
    let mut window = Window::open(agent).await?;
    let key = FakeKey::new(7);
    let user = account(&mut window, server, SignInPolicy::PasswordAndKey, &key).await?;
    let (request_id, challenge) = challenged(&mut window, &user.username).await?;

    let mut stranger = Window::open(agent).await?;
    let foreign = answer(&mut stranger, &challenge, &key.credential_id, &key.prf).await?;
    assert!(
        foreign.is_err(),
        "a window that was not asked answered: {foreign:?}"
    );
    let other_key = answer(&mut window, &challenge, b"another-credential", &key.prf).await?;
    assert!(other_key.is_err(), "an unlisted credential answered");
    let short = answer(&mut window, &challenge, &key.credential_id, &[7; 16]).await?;
    assert!(short.is_err(), "a 16-byte PRF output answered");

    // None of those spent it.
    answer(&mut window, &challenge, &key.credential_id, &key.prf).await??;
    let second = answer(&mut window, &challenge, &key.credential_id, &key.prf).await?;
    assert!(second.is_err(), "a second answer counted");

    let signed_in = window.next_of(request_id).await?;
    assert!(
        matches!(signed_in, InternalServiceResponse::ConnectSuccess(_)),
        "the first valid answer did not sign in: {signed_in:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_declined_challenge_fails_the_sign_in_at_once() -> Result<(), Box<dyn Error>> {
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
    let (request_id, challenge) = challenged(&mut window, &user.username).await?;

    let started = Instant::now();
    window
        .send(InternalServiceRequest::SecurityKeyDecline {
            request_id,
            challenge_id: challenge.challenge_id,
            reason: "The key has no PRF support".to_string(),
        })
        .await?;
    let declined = window.next_of(request_id).await?;
    assert!(matches!(
        declined,
        InternalServiceResponse::SecurityKeyAnswerSuccess(_)
    ));
    let failed = window.next_of(request_id).await?;
    assert!(
        matches!(failed, InternalServiceResponse::ConnectFailure(ref f) if f.message.contains("no PRF support")),
        "the decline did not fail the sign-in with its reason: {failed:?}"
    );
    assert!(
        started.elapsed() < KEY_WINDOW / 2,
        "the decline waited for the deadline"
    );
    Ok(())
}

#[tokio::test]
async fn an_unanswered_challenge_fails_at_the_sdks_deadline_and_no_later(
) -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_server(Server::PostQuantum);
    let mut window = Window::open(spawn_agent().await?).await?;
    let key = FakeKey::new(7);
    let user = account(&mut window, server, SignInPolicy::PasswordAndKey, &key).await?;
    let started = Instant::now();
    let (request_id, challenge) = challenged(&mut window, &user.username).await?;
    assert_eq!(challenge.expires_in_ms, KEY_WINDOW.as_millis() as u64);

    let failed = window.next_of(request_id).await?;
    let waited = started.elapsed();
    assert!(
        matches!(failed, InternalServiceResponse::ConnectFailure(_)),
        "an unanswered challenge signed in: {failed:?}"
    );
    assert!(
        waited >= KEY_WINDOW,
        "failed after {waited:?}, before the deadline"
    );
    assert!(
        waited < KEY_WINDOW + Duration::from_secs(15),
        "waited {waited:?}"
    );

    let late = answer(&mut window, &challenge, &key.credential_id, &key.prf).await?;
    assert!(late.is_err(), "an answer after the deadline was taken");
    Ok(())
}

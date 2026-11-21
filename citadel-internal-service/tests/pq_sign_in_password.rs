//! Post-quantum sign-in through the agent: registering returns the recovery phrase, the password
//! signs in, and a live session is still handed only to someone who knows that password.
//!
//! A server without post-quantum settings runs the legacy sign-in, and the same requests work
//! against it: that is the version gate's fallback, seen from the agent.

use citadel_internal_service_test_common::pq::{
    spawn_agent, spawn_server, username, Server, PASSWORD,
};
use citadel_internal_service_test_common::pq_window::{Offer, Registered, Window};
use citadel_internal_service_test_common::setup_log;
use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
use citadel_sdk::prelude::RecoverySeed;
use std::error::Error;
use uuid::Uuid;

fn password(password: &str) -> Offer {
    Offer {
        password: Some(password.to_string()),
        ..Default::default()
    }
}

/// A server that recovers accounts issues the 24-word phrase: the agent named the server's
/// canonical domain. Before, it named none, and the account had no recovery credential.
fn assert_a_phrase_and_no_codes(registered: &Registered) {
    let typed = registered.typed_phrase();
    assert_eq!(typed.split(' ').count(), 24, "the phrase is 24 words");
    assert!(
        RecoverySeed::parse(&typed).is_ok(),
        "the phrase does not read back"
    );
}

#[tokio::test]
async fn a_registration_returns_the_recovery_phrase_and_the_password_signs_in(
) -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_server(Server::PostQuantum);
    let mut window = Window::open(spawn_agent().await?).await?;
    let user = username("pq");

    let registered = window.register(server, &user, PASSWORD).await??;
    assert_a_phrase_and_no_codes(&registered);
    let registered = registered.cid;

    let signed_in = window.connect(server, &user, &password(PASSWORD)).await?;
    let InternalServiceResponse::ConnectSuccess(success) = signed_in.response else {
        panic!("the password did not sign in: {:?}", signed_in.response);
    };
    assert_eq!(success.cid, registered);
    assert!(
        signed_in.challenges.is_empty(),
        "a password account asked for a key"
    );
    Ok(())
}

/// The live branch of `Connect` never reaches the server, so the agent itself must tell the
/// password apart. For a post-quantum account the SDK's client-side hash ignores the password
/// (there is nothing to hash: the password is proven by a factor), so a fingerprint taken from
/// it would admit anyone who knew the username.
#[tokio::test]
async fn a_live_post_quantum_session_is_not_handed_to_a_wrong_password(
) -> Result<(), Box<dyn Error>> {
    a_live_session_is_handed_only_to_its_password(Server::PostQuantum).await
}

async fn a_live_session_is_handed_only_to_its_password(kind: Server) -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_server(kind);
    let agent = spawn_agent().await?;
    let mut first = Window::open(agent).await?;
    let user = username("live");
    let registered = first.register(server, &user, PASSWORD).await??;
    assert_a_phrase_and_no_codes(&registered);
    let cid = first.sign_in(server, &user, &password(PASSWORD)).await??;

    let mut second = Window::open(agent).await?;
    let wrong = second
        .connect(server, &user, &password("not the password"))
        .await?;
    assert!(
        matches!(wrong.response, InternalServiceResponse::ConnectFailure(ref f) if f.cid == 0),
        "a wrong password was handed the live session: {:?}",
        wrong.response
    );
    let right = second.connect(server, &user, &password(PASSWORD)).await?;
    assert!(
        matches!(right.response, InternalServiceResponse::SessionAlreadyActive(ref a) if a.cid == cid),
        "the right password was not handed the live session: {:?}",
        right.response
    );
    Ok(())
}

/// `connect_after_register`, as the UI registers: the phrase arrives first, under the
/// registration's request id, and the connect's own answer follows.
#[tokio::test]
async fn registering_and_connecting_at_once_still_delivers_the_phrase() -> Result<(), Box<dyn Error>>
{
    setup_log();
    let server = spawn_server(Server::PostQuantum);
    let mut window = Window::open(spawn_agent().await?).await?;
    let user = username("reg");
    let request_id = Uuid::new_v4();
    window
        .send(InternalServiceRequest::Register {
            request_id,
            server_addr: server.to_string(),
            full_name: user.clone(),
            username: user.clone(),
            proposed_password: PASSWORD.into(),
            connect_after_register: true,
            session_security_settings: citadel_sdk::prelude::default_session_security_settings(),
            server_password: None,
            admission_token: None,
        })
        .await?;
    let first = window.answer_of(request_id, None).await?.response;
    let InternalServiceResponse::RegisterSuccess(registered) = first else {
        panic!("the phrase did not come first: {first:?}");
    };
    assert_eq!(
        registered.recovery_phrase.0.len(),
        24,
        "the phrase did not come with the registration"
    );
    let second = window.answer_of(request_id, None).await?.response;
    assert!(
        matches!(second, InternalServiceResponse::ConnectSuccess(ref s) if s.cid == registered.cid),
        "the connect did not follow: {second:?}"
    );
    Ok(())
}

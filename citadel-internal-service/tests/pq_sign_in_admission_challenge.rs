//! A server's `admission_required` refusal publishes the check to show; the agent passes it to
//! the window as `admission_challenge`, next to `reason_code`, on a ConnectFailure and a
//! RegisterFailure. A refused token is not a request for the check, so it carries none.

use citadel_internal_service_test_common::pq::{
    spawn_agent, spawn_guarded_server, username, GOOD_TOKEN, IDENTITY, PASSWORD, SITE_KEY,
};
use citadel_internal_service_test_common::pq_window::{Offer, Window};
use citadel_internal_service_test_common::setup_log;
use citadel_internal_service_types::{
    AdmissionChallengeWire, AdmissionProviderWire, FailureReason, InternalServiceRequest,
    InternalServiceResponse, RegisterFailure,
};
use std::error::Error;
use std::net::SocketAddr;
use uuid::Uuid;

fn published() -> Option<AdmissionChallengeWire> {
    Some(AdmissionChallengeWire {
        provider: AdmissionProviderWire::Turnstile,
        site_key: SITE_KEY.to_string(),
        identity: Some(IDENTITY.to_string()),
    })
}

async fn refused_registration(
    window: &mut Window,
    server: SocketAddr,
    user: &str,
    token: Option<&str>,
) -> Result<RegisterFailure, Box<dyn Error>> {
    let request_id = Uuid::new_v4();
    window
        .send(InternalServiceRequest::Register {
            request_id,
            server_addr: server.to_string(),
            full_name: user.to_string(),
            username: user.to_string(),
            proposed_password: PASSWORD.into(),
            connect_after_register: false,
            session_security_settings: Default::default(),
            server_password: None,
            admission_token: token.map(str::to_string),
        })
        .await?;
    match window.answer_of(request_id, None).await?.response {
        InternalServiceResponse::RegisterFailure(failure) => Ok(failure),
        other => Err(format!("the registration was not refused: {other:?}").into()),
    }
}

fn password(token: Option<&str>) -> Offer {
    Offer {
        password: Some(PASSWORD.to_string()),
        admission: token.map(str::to_string),
        ..Default::default()
    }
}

#[tokio::test]
async fn the_published_check_reaches_a_refused_registration_and_sign_in(
) -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_guarded_server();
    let mut window = Window::open(spawn_agent().await?).await?;
    let user = username("admchal");

    let missing = refused_registration(&mut window, server, &user, None).await?;
    assert_eq!(missing.reason_code, Some(FailureReason::AdmissionRequired));
    assert_eq!(missing.admission_challenge, published(), "register lost it");
    let forged = refused_registration(&mut window, server, &user, Some("forged")).await?;
    assert_eq!(forged.reason_code, Some(FailureReason::AdmissionFailed));
    assert_eq!(forged.admission_challenge, None);
    window
        .register_admitted(server, &user, PASSWORD, Some(GOOD_TOKEN))
        .await?
        .map_err(|(message, _)| message)?;

    let refused = window.connect(&user, &password(None)).await?.response;
    let InternalServiceResponse::ConnectFailure(failure) = refused else {
        panic!("a sign-in was admitted without a token: {refused:?}");
    };
    assert_eq!(failure.reason_code, Some(FailureReason::AdmissionRequired));
    assert_eq!(failure.admission_challenge, published(), "sign-in lost it");
    let refused = window
        .connect(&user, &password(Some("forged")))
        .await?
        .response;
    let InternalServiceResponse::ConnectFailure(failure) = refused else {
        panic!("a forged token was admitted: {refused:?}");
    };
    assert_eq!(failure.admission_challenge, None);
    Ok(())
}

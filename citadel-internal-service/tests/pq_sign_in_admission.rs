//! A workspace that requires a Turnstile token for each fresh sign-in and registration. The
//! window's token rides in `Connect.admission_token` / `Register.admission_token`; a refusal is
//! answered with a machine-readable `reason_code`, so the UI knows to show or reset the widget.

use citadel_internal_service_test_common::pq::{
    spawn_agent, spawn_guarded_server, username, GOOD_TOKEN, PASSWORD,
};
use citadel_internal_service_test_common::pq_window::{Offer, Window};
use citadel_internal_service_test_common::setup_log;
use citadel_internal_service_types::{
    FailureReason, InternalServiceRequest, InternalServiceResponse,
};
use std::error::Error;
use uuid::Uuid;

fn password(token: Option<&str>) -> Offer {
    Offer {
        password: Some(PASSWORD.to_string()),
        admission: token.map(str::to_string),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_registration_and_a_sign_in_need_a_good_token() -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_guarded_server();
    let mut window = Window::open(spawn_agent().await?).await?;
    let user = username("adm");

    for (token, reason) in [
        (None, FailureReason::AdmissionRequired),
        (Some("forged"), FailureReason::AdmissionFailed),
    ] {
        let refused = window
            .register_admitted(server, &user, PASSWORD, token)
            .await?;
        let (_, code) = refused.expect_err("a registration was admitted without a good token");
        assert_eq!(code, Some(reason), "{token:?}");
    }
    window
        .register_admitted(server, &user, PASSWORD, Some(GOOD_TOKEN))
        .await?
        .map_err(|(message, _)| message)?;

    for (token, reason) in [
        (None, FailureReason::AdmissionRequired),
        (Some("forged"), FailureReason::AdmissionFailed),
    ] {
        let refused = window.connect(&user, &password(token)).await?.response;
        let InternalServiceResponse::ConnectFailure(failure) = refused else {
            panic!("a sign-in was admitted without a good token: {refused:?}");
        };
        assert_eq!(failure.reason_code, Some(reason), "{token:?}");
    }
    let signed_in = window.sign_in(&user, &password(Some(GOOD_TOKEN))).await?;
    assert!(signed_in.is_ok(), "a good token was refused: {signed_in:?}");
    Ok(())
}

/// A wrong password is not an admission problem: no `reason_code`.
#[tokio::test]
async fn a_wrong_password_carries_no_reason_code() -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_guarded_server();
    let mut window = Window::open(spawn_agent().await?).await?;
    let user = username("admpw");
    window
        .register_admitted(server, &user, PASSWORD, Some(GOOD_TOKEN))
        .await?
        .map_err(|(message, _)| message)?;
    let offer = Offer {
        password: Some("not the password".to_string()),
        admission: Some(GOOD_TOKEN.to_string()),
        ..Default::default()
    };
    let refused = window.connect(&user, &offer).await?.response;
    let InternalServiceResponse::ConnectFailure(failure) = refused else {
        panic!("a wrong password signed in: {refused:?}");
    };
    assert_eq!(failure.reason_code, None);
    Ok(())
}

/// A Turnstile token is single-use, so the registration's cannot also admit the connect that
/// `connect_after_register` dispatches: that connect asks the window for its own.
#[tokio::test]
async fn connect_after_register_asks_for_a_fresh_sign_in_token() -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_guarded_server();
    let mut window = Window::open(spawn_agent().await?).await?;
    let user = username("admcar");
    let request_id = Uuid::new_v4();
    window
        .send(InternalServiceRequest::Register {
            request_id,
            server_addr: server.to_string(),
            full_name: user.clone(),
            username: user.clone(),
            proposed_password: PASSWORD.into(),
            connect_after_register: true,
            session_security_settings: Default::default(),
            server_password: None,
            admission_token: Some(GOOD_TOKEN.to_string()),
        })
        .await?;
    let registered = window.answer_of(request_id, None).await?.response;
    assert!(
        matches!(registered, InternalServiceResponse::RegisterSuccess(_)),
        "{registered:?}"
    );
    let connect = window.answer_of(request_id, None).await?.response;
    let InternalServiceResponse::ConnectFailure(failure) = connect else {
        panic!("the connect reused the registration's token: {connect:?}");
    };
    assert_eq!(failure.reason_code, Some(FailureReason::AdmissionRequired));
    let signed_in = window.sign_in(&user, &password(Some(GOOD_TOKEN))).await?;
    assert!(signed_in.is_ok(), "{signed_in:?}");
    Ok(())
}

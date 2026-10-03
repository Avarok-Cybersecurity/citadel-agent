//! The wire shape of the sign-in fields, as the browser's JSON carries it.

use super::*;

/// A `Connect` as a client from before post-quantum sign-in sends it: a password, and neither
/// `security_key` nor `recovery_code` (nor `server_password`, which the UI never sends).
fn legacy_connect_json() -> String {
    let current = InternalServiceRequest::Connect {
        request_id: Uuid::new_v4(),
        username: "alice".to_string(),
        password: Some(SecBuffer::from("hunter2")),
        security_key: true,
        recovery_code: None,
        connect_mode: ConnectMode::Standard { force_login: false },
        udp_mode: UdpMode::Enabled,
        keep_alive_timeout: None,
        session_security_settings: SessionSecuritySettings::default(),
        server_password: None,
    };
    let mut value = serde_json::to_value(&current).unwrap();
    let fields = value["Connect"].as_object_mut().unwrap();
    for added in ["security_key", "recovery_code", "server_password"] {
        assert!(fields.remove(added).is_some(), "{added} is not a field");
    }
    value.to_string()
}

#[test]
fn an_older_clients_connect_still_parses_as_a_password_sign_in() {
    let request: InternalServiceRequest = serde_json::from_str(&legacy_connect_json()).unwrap();
    let InternalServiceRequest::Connect {
        password,
        security_key,
        recovery_code,
        ..
    } = request
    else {
        panic!("not a Connect: {request:?}");
    };
    assert_eq!(password.expect("the password").as_ref(), b"hunter2");
    assert!(
        !security_key,
        "an older client cannot answer a key challenge"
    );
    assert!(recovery_code.is_none());
}

#[test]
fn a_prf_output_never_reaches_a_debug_string() {
    let answer = InternalServiceRequest::SecurityKeyAnswer {
        request_id: Uuid::nil(),
        challenge_id: Uuid::nil(),
        credential_id: vec![1, 2, 3],
        prf_output: SecBuffer::from(vec![0xAB; 32]),
    };
    let printed = format!("{answer:?}");
    assert!(!printed.contains("171"), "{printed}");
    assert!(!printed.to_lowercase().contains("ab, ab"), "{printed}");
}

#[test]
fn a_registers_recovery_codes_never_reach_a_debug_string() {
    let success = InternalServiceResponse::RegisterSuccess(RegisterSuccess {
        cid: 7,
        request_id: None,
        recovery_codes: RecoveryCodes(vec!["7H3Q-K2M9-PX4D-W8RT".to_string()]),
    });
    assert!(!format!("{success:?}").contains("7H3Q"));
}

#[test]
fn the_new_notification_is_a_notification_and_the_failures_are_errors() {
    let challenge = InternalServiceResponse::SecurityKeyChallengeNotification(
        SecurityKeyChallengeNotification {
            cid: 0,
            request_id: None,
            challenge_id: Uuid::nil(),
            purpose: SecurityKeyPurpose::SignIn,
            allowed_credential_ids: vec![],
            prf_salt: vec![0; 32],
            expires_in_ms: 60_000,
        },
    );
    assert!(challenge.is_notification());
    let refused = InternalServiceResponse::SecurityKeyAnswerFailure(SecurityKeyAnswerFailure {
        cid: 0,
        request_id: None,
        challenge_id: Uuid::nil(),
        message: String::new(),
    });
    assert!(refused.is_error());
    let failed = InternalServiceResponse::SignInManagementFailure(SignInManagementFailure {
        cid: 0,
        request_id: None,
        message: String::new(),
    });
    assert!(failed.is_error());
}

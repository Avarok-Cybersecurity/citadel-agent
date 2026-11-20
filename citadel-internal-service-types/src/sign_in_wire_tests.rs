//! The wire shape of the sign-in fields, as the browser's JSON carries it.

use super::*;

/// A `Connect` that names only a password: no `security_key`, no `recovery_phrase` (nor
/// `server_password`, which the UI never sends). Nor `server_address`: it parses, empty, so the
/// agent can answer that page (connect.rs).
fn legacy_connect_json() -> String {
    let current = InternalServiceRequest::Connect {
        request_id: Uuid::new_v4(),
        username: "alice".to_string(),
        server_address: "bench.work.avarok.net".to_string(),
        password: Some(SecBuffer::from("hunter2")),
        security_key: true,
        recovery_phrase: None,
        connect_mode: ConnectMode::Standard { force_login: false },
        udp_mode: UdpMode::Enabled,
        keep_alive_timeout: None,
        session_security_settings: SessionSecuritySettings::default(),
        server_password: None,
        admission_token: None,
    };
    let mut value = serde_json::to_value(&current).unwrap();
    let fields = value["Connect"].as_object_mut().unwrap();
    for added in [
        "security_key",
        "recovery_phrase",
        "server_password",
        "admission_token",
        "server_address",
    ] {
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
        recovery_phrase,
        server_address,
        ..
    } = request
    else {
        panic!("not a Connect: {request:?}");
    };
    // Parsed rather than dropped, so the agent answers it: an empty server is refused at once.
    assert_eq!(server_address, "");
    assert_eq!(password.expect("the password").as_ref(), b"hunter2");
    assert!(
        !security_key,
        "an older client cannot answer a key challenge"
    );
    assert!(recovery_phrase.is_none());
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
fn a_registrations_phrase_never_reaches_a_debug_string() {
    let success = InternalServiceResponse::RegisterSuccess(RegisterSuccess {
        cid: 7,
        request_id: None,
        recovery_phrase: RecoveryCodes(vec!["zebra".to_string(); 24]),
        identity: None,
    });
    let printed = format!("{success:?}");
    assert!(!printed.contains("zebra"), "{printed}");
}

#[test]
fn a_typed_phrase_never_reaches_a_debug_string() {
    let recover = InternalServiceRequest::RecoverAccount {
        request_id: Uuid::nil(),
        server_addr: "acme.example".into(),
        username: "alice".into(),
        phrase: SecBuffer::from("zebra zebra zebra"),
        device_label: "laptop".into(),
        admission_token: Some("turnstile-secret".into()),
        session_security_settings: Default::default(),
        server_password: None,
    };
    let printed = format!("{recover:?}");
    assert!(!printed.contains("zebra"), "{printed}");
    assert!(!printed.contains("turnstile-secret"), "{printed}");
}

/// A registration always carries its phrase: an answer without one is not a registration.
#[test]
fn a_registration_without_a_phrase_does_not_parse() {
    let json = r#"{"cid":1,"request_id":null,"recovery_codes":["A"]}"#;
    assert!(serde_json::from_str::<RegisterSuccess>(json).is_err());
}

#[test]
fn the_new_notification_is_a_notification_and_the_failures_are_errors() {
    let challenge = InternalServiceResponse::SecurityKeyChallengeNotification(
        SecurityKeyChallengeNotification {
            cid: 0,
            request_id: None,
            challenge_id: Uuid::nil(),
            purpose: SecurityKeyPurpose::SignIn,
            rp_id: Some("work.avarok.net".to_string()),
            ceremony: SecurityKeyCeremony::Authenticate {
                credential_ids: vec![],
            },
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
        reason: None,
    });
    assert!(failed.is_error());
}

#[test]
fn an_admission_token_never_reaches_a_debug_string() {
    let connect = InternalServiceRequest::Connect {
        request_id: Uuid::nil(),
        username: "alice".to_string(),
        server_address: "bench.work.avarok.net".to_string(),
        password: None,
        security_key: false,
        recovery_phrase: None,
        admission_token: Some("0.turnstile-secret".to_string()),
        connect_mode: ConnectMode::Standard { force_login: false },
        udp_mode: UdpMode::Enabled,
        keep_alive_timeout: None,
        session_security_settings: SessionSecuritySettings::default(),
        server_password: None,
    };
    let printed = format!("{connect:?}");
    assert!(!printed.contains("turnstile-secret"), "{printed}");
    assert!(printed.contains("redacted"), "{printed}");
}

#[test]
fn reason_codes_cross_the_wire_in_snake_case() {
    let json = serde_json::to_string(&FailureReason::AdmissionRequired).unwrap();
    assert_eq!(json, "\"admission_required\"");
    let json = serde_json::to_string(&FailureReason::AdmissionFailed).unwrap();
    assert_eq!(json, "\"admission_failed\"");
}

#[test]
fn a_key_challenge_and_its_answer_never_print_their_bytes() {
    let credential = b"yubikey-5-credential-id".to_vec();
    let salt = vec![0x5a; 32];
    let challenge = SecurityKeyChallengeNotification {
        cid: 1,
        request_id: None,
        challenge_id: Uuid::nil(),
        purpose: SecurityKeyPurpose::SignIn,
        rp_id: Some("work.avarok.net".to_string()),
        ceremony: SecurityKeyCeremony::Authenticate {
            credential_ids: vec![credential.clone()],
        },
        prf_salt: salt.clone(),
        expires_in_ms: 60_000,
    };
    let answer = InternalServiceRequest::SecurityKeyAnswer {
        request_id: Uuid::nil(),
        challenge_id: Uuid::nil(),
        credential_id: credential.clone(),
        prf_output: SecBuffer::from(vec![1; 32]),
    };
    for printed in [format!("{challenge:?}"), format!("{answer:?}")] {
        // "121, 117, 98, 105, 107": the credential's first bytes, as `{:?}` lists them.
        let leading = format!("{:?}", &credential[..5]);
        let leading = leading.trim_start_matches('[').trim_end_matches(']');
        assert!(!printed.contains(leading), "{printed}");
        assert!(!printed.contains("90, 90"), "{printed}");
    }
}

#[test]
fn a_reason_from_a_newer_agent_decodes_as_unknown_and_keeps_its_failure() {
    let reason: FailureReason = serde_json::from_str("\"some_reason_from_the_future\"").unwrap();
    assert_eq!(reason, FailureReason::Unknown);
    let failure: RegisterFailure = serde_json::from_str(
        r#"{"cid":0,"message":"refused","request_id":null,"reason_code":"some_reason_from_the_future"}"#,
    )
    .unwrap();
    assert_eq!(failure.reason_code, Some(FailureReason::Unknown));
    assert_eq!(failure.message, "refused");
}

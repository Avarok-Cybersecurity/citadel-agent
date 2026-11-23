//! A reconnect whose resume credential the server will not take, and one refused over the
//! protocol version, stop at once and say which: no further attempt can end differently, and
//! each asks something different of the user.

use super::attempt::sign_in_again;
use super::policy::*;
use super::Reauth;
use crate::kernel::resume::test_support::{credential, ISSUED_AT};
use citadel_internal_service_types::FailureReason;
use citadel_io::ErrorCode;
use citadel_sdk::prelude::{AuthenticationRequest, SecBuffer};
use std::time::Duration;

const NAMED: [(ErrorCode, FailureReason); 3] = [
    (
        ErrorCode::ResumeCredentialExpired,
        FailureReason::ResumeCredentialExpired,
    ),
    (
        ErrorCode::ResumeCredentialRevoked,
        FailureReason::ResumeCredentialRevoked,
    ),
    (
        ErrorCode::ProtocolVersionUnsupported,
        FailureReason::ProtocolVersionUnsupported,
    ),
];

/// Control: take a code out of `REFUSALS`, and it is retried for ten minutes.
#[test]
fn each_is_definite_and_is_not_retried() {
    for (code, _) in NAMED {
        let kind = classify(code, code.raw_string());
        assert_eq!(kind, FailureKind::Refused, "{code:?}");
        assert_eq!(
            SERVER_RECONNECT.after_failure(0, Duration::ZERO, kind),
            Next::GiveUp(GiveUp::Refused),
            "{code:?} was retried"
        );
    }
}

/// Control: map `ResumeCredentialRevoked` to the expired reason.
#[test]
fn each_reaches_the_window_under_its_own_reason() {
    for (code, reason) in NAMED {
        let named = refusal(code, code.raw_string()).and_then(refusal_reason);
        assert_eq!(named, Some(reason), "{code:?}");
    }
    assert_ne!(
        refusal_reason(ErrorCode::ResumeCredentialExpired),
        refusal_reason(ErrorCode::ResumeCredentialRevoked),
    );
    // Neither is the plain refused sign-in, which asks for nothing in particular.
    for (_, reason) in NAMED {
        assert_ne!(reason, FailureReason::SignInRefused);
    }
}

/// An older server's refusal reaches an older SDK as text alone; it is still this refusal.
#[test]
fn a_version_refusal_sent_as_text_is_recognised() {
    let text = "The other node speaks protocol 0.12.5; this node takes part only with 0.12.6 \
                or a later patch of that release. Update the older node";
    for code in [ErrorCode::Generic, ErrorCode::RemoteConnectFailed] {
        assert_eq!(classify(code, text), FailureKind::Refused, "{code:?}");
        assert_eq!(
            refusal(code, text).and_then(refusal_reason),
            Some(FailureReason::ProtocolVersionUnsupported)
        );
    }
    assert_eq!(
        classify(ErrorCode::Generic, "no route to host"),
        FailureKind::Transient
    );
}

/// Control: sign in again with the password when the session holds a credential.
#[tokio::test]
async fn a_session_signs_in_again_with_what_it_kept_and_never_guesses() {
    let held = Reauth::Resume(credential(3_600).await);
    let Ok(AuthenticationRequest::SignIn { factors, .. }) = sign_in_again("alice".into(), &held)
    else {
        panic!("a resume credential signs in as itself");
    };
    let offered = factors.resume.expect("the credential is offered");
    assert_eq!(offered.valid_until_utc(), ISSUED_AT + 3_600_000);
    assert!(
        factors.password.is_none(),
        "a password was offered beside it"
    );

    let password = Reauth::Password(SecBuffer::from("pw"));
    assert!(matches!(
        sign_in_again("alice".into(), &password),
        Ok(AuthenticationRequest::Credentialed { .. })
    ));
    let needs = Reauth::NeedsUser("sign in again");
    assert_eq!(
        sign_in_again("alice".into(), &needs).err(),
        Some("sign in again")
    );
}

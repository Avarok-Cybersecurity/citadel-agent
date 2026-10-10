//! A reconnect the server's admission check (Turnstile) refuses stops at once and says why.
//! It used to count as transient: 24 attempts over ten minutes, each asked the same check.

use super::policy::*;
use crate::kernel::sign_in::failure_reason;
use citadel_internal_service_types::FailureReason;
use citadel_io::ErrorCode;
use std::time::Duration;

/// Each refusal, as the server renders it, and what the UI is told.
const ADMISSION: [(ErrorCode, &str, FailureReason); 2] = [
    (
        ErrorCode::PqSignInAdmissionRequired,
        "This workspace needs a verification check before you sign in",
        FailureReason::AdmissionRequired,
    ),
    (
        ErrorCode::PqSignInAdmissionFailed,
        "The verification check failed: invalid-input-response",
        FailureReason::AdmissionFailed,
    ),
];

/// As the agent logged it: "(Transient, PqSignInAdmissionRequired): This workspace needs a
/// verification check before you sign in".
#[test]
fn an_admission_refusal_is_not_retried() {
    for (code, message, _) in ADMISSION {
        let kind = classify(code, message);
        assert_eq!(kind, FailureKind::Refused, "{code:?}");
        assert_eq!(
            SERVER_RECONNECT.after_failure(0, Duration::ZERO, kind),
            Next::GiveUp(GiveUp::Refused),
            "{code:?} was retried"
        );
    }
}

#[test]
fn the_give_up_carries_what_the_ui_must_ask_for() {
    for (code, message, reason) in ADMISSION {
        assert_eq!(
            refusal(code, message).and_then(failure_reason),
            Some(reason)
        );
        let relayed = refusal(ErrorCode::Generic, message);
        assert_eq!(relayed, Some(code), "the server's rendered {code:?}");
        assert_eq!(relayed.and_then(failure_reason), Some(reason));
    }
}

/// Negative control: what a retry can fix still is retried, and carries no reason.
#[test]
fn an_unreachable_server_is_still_retried_and_names_no_reason() {
    let message = "Connection refused (os error 61)";
    for code in [ErrorCode::Generic, ErrorCode::RemoteConnectFailed] {
        assert_eq!(classify(code, message), FailureKind::Transient);
        assert_eq!(refusal(code, message), None);
    }
    let mentioned = "timed out after This workspace needs a verification check before you sign in";
    assert_eq!(refusal(ErrorCode::Generic, mentioned), None);
}

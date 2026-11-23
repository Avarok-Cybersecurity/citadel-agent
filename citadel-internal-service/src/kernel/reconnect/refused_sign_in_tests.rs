//! A reconnect whose sign-in the server refuses stops at once and says so; one the server
//! never answered says that instead. "Authentication failed" used to count as transient:
//! 24 attempts over ten minutes, every one refused the same way, with the window showing
//! "Reconnecting" throughout.

use super::policy::*;
use crate::kernel::sign_in::failure_reason;
use citadel_internal_service_types::FailureReason;
use citadel_io::ErrorCode;
use std::time::Duration;

/// As the agent logged it: "(Transient, Generic): Authentication failed".
const AS_LOGGED: &str = "Authentication failed";

#[test]
fn a_refused_sign_in_is_not_retried() {
    assert_eq!(ErrorCode::PqSignInFailed.raw_string(), AS_LOGGED);
    let relayed = [ErrorCode::Generic, ErrorCode::RemoteConnectFailed];
    for code in relayed.into_iter().chain([ErrorCode::PqSignInFailed]) {
        let kind = classify(code, AS_LOGGED);
        assert_eq!(kind, FailureKind::Refused, "{code:?}");
        assert_eq!(
            SERVER_RECONNECT.after_failure(0, Duration::ZERO, kind),
            Next::GiveUp(GiveUp::Refused),
            "{code:?} was retried"
        );
    }
}

#[test]
fn the_give_up_tells_the_window_to_sign_in_again() {
    for code in [ErrorCode::Generic, ErrorCode::PqSignInFailed] {
        assert_eq!(
            refusal(code, AS_LOGGED).and_then(refusal_reason),
            Some(FailureReason::SignInRefused),
            "{code:?}"
        );
    }
    // The other refusals keep the reason they always carried.
    assert_eq!(
        refusal_reason(ErrorCode::PqSignInAdmissionRequired),
        Some(FailureReason::AdmissionRequired)
    );
    assert_eq!(refusal_reason(ErrorCode::PreconnectRefused), None);
}

/// A Connect a user typed is refused with the same code for a mistyped password; the form
/// words that itself, and a page older than this agent would read a new reason as "reload".
#[test]
fn a_refused_connect_still_carries_no_reason() {
    assert_eq!(failure_reason(ErrorCode::PqSignInFailed), None);
}

/// Negative control: a failure that only mentions the refusal, or is none, is still retried.
#[test]
fn a_mention_of_the_refusal_is_still_transient() {
    for message in [
        "timed out after Authentication failed",
        "Connection refused (os error 61)",
    ] {
        for code in [ErrorCode::Generic, ErrorCode::RemoteConnectFailed] {
            assert_eq!(classify(code, message), FailureKind::Transient, "{message}");
            assert_eq!(refusal(code, message), None, "{message}");
        }
    }
}

#[test]
fn only_a_server_that_never_answered_is_reported_unreachable() {
    assert_eq!(
        out_of_time_reason(FailureKind::Transient),
        Some(FailureReason::ServerUnreachable)
    );
    for answered in [FailureKind::ServerHoldsSession, FailureKind::Refused] {
        assert_eq!(out_of_time_reason(answered), None, "{answered:?}");
    }
}

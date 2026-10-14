//! How a peer request's SDK failure is logged. A request about a session the SDK no longer
//! holds is the caller's stale state, not a fault of the agent: the caller is still told in
//! the failure response, and the log says it at debug.

use citadel_io::ErrorCode;
use citadel_sdk::logging::{debug, error};
use citadel_sdk::prelude::NetworkError;

/// The SDK has no session for the CID the request names.
pub(crate) fn names_unknown_session(err: &NetworkError) -> bool {
    matches!(
        err.code(),
        ErrorCode::SessionManagerDispatchSessionNotFound
            | ErrorCode::SessionManagerSessionNotFound
            | ErrorCode::SessionManagerNotActiveSession
    )
}

/// Logs `what` failing with `err` and returns the text the failure response carries.
pub(crate) fn log_failure(what: &str, err: NetworkError) -> String {
    let unknown_session = names_unknown_session(&err);
    let text = err.into_string();
    if unknown_session {
        debug!(target: "citadel", "{what}: {text} (the caller's session is gone)");
    } else {
        error!(target: "citadel", "{what}: {text}");
    }
    text
}

/// A dial's failure. A window's dial is answered to the user and logged as an error; the
/// supervisor's own dial (no request) repeats every backoff, and says it once, at WARN,
/// where the supervisor reports the peer as not answering.
pub(crate) fn log_connect_failure(request_id: Option<uuid::Uuid>, text: &str) {
    if request_id.is_some() {
        error!(target: "citadel", "[PeerConnect] connect_to_peer_custom FAILED: {text}");
    } else {
        debug!(target: "citadel", "[PeerConnect] connect_to_peer_custom FAILED: {text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_the_sdk_does_not_hold_is_not_an_agent_fault() {
        assert!(names_unknown_session(&citadel_io::error!(
            ErrorCode::SessionManagerDispatchSessionNotFound,
            7_u64
        )));
        assert!(names_unknown_session(&citadel_io::error!(
            ErrorCode::SessionManagerSessionNotFound,
            7_u64
        )));
        assert!(names_unknown_session(&citadel_io::error!(
            ErrorCode::SessionManagerNotActiveSession,
            7_u64
        )));
    }

    #[test]
    fn other_failures_stay_errors() {
        assert!(!names_unknown_session(&citadel_io::error!(
            ErrorCode::Generic,
            "boom"
        )));
    }

    #[test]
    fn the_response_text_is_the_sdks_message_unchanged() {
        let err = citadel_io::error!(ErrorCode::SessionManagerDispatchSessionNotFound, 7_u64);
        assert_eq!(
            log_failure("[ListAllPeers] FAILURE for cid=7", err),
            "Session for 7 not found in session manager. Failed to dispatch peer command"
        );
    }
}

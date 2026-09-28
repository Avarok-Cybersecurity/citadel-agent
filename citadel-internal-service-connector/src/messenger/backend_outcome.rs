//! What an agent's answer to a LocalDB request means.
//!
//! Moved verbatim from backend.rs when storage became a trait: these decisions
//! belong to the WebSocket store (`backend_ws.rs`), which is the only store
//! that receives `InternalServiceResponse`s.
use crate::messenger::WrappedMessage;
use citadel_internal_service_types::{InternalServiceResponse, KEY_NOT_FOUND};
use intersession_layer_messaging::BackendError;

/// Did that write actually happen?
///
/// Separated from the socket so the decision can be tested exhaustively against
/// real response values instead of a mocked agent, and so the three write paths
/// share ONE answer. They did not: `update_map` and `store_value` asked
/// `wait_for_response(..).is_some()` — the presence of a reply — while
/// `store_values_batched` matched the variant and carried a comment saying all
/// three "must agree". A `LocalDBSetKVFailure` is a reply, and the agent sends
/// one on a backend error, on a failed `propose_target`, and on the ownership-gate
/// refusal. So a refused write returned `Ok(())`, the map read as stored, ILM read
/// as queued, and the sender saw a message as sent that nothing would retransmit.
pub(crate) fn write_outcome(
    response: Option<InternalServiceResponse>,
    what: &str,
) -> Result<(), BackendError<WrappedMessage>> {
    match response {
        Some(InternalServiceResponse::LocalDBSetKVSuccess(_)) => Ok(()),
        Some(other) => Err(BackendError::StorageError(format!(
            "Writing {what} was refused or failed: {other:?}"
        ))),
        // A timeout is not a success either; the caller must be able to retry.
        None => Err(BackendError::StorageError(format!(
            "Timed out writing {what}; the change may not be stored"
        ))),
    }
}

/// Absent, present, or unreadable — three outcomes, not two.
///
/// `load_values_batched` folded every non-success into `None`, so a backend
/// error read as "no such key". `MessageTracker::new` then starts with an empty
/// delivery frontier: already-received messages are re-delivered, ACK state is
/// reset and the next-id counter restarts, on an error that should have failed
/// initialisation. `get_map` draws this distinction and explains it; this is the
/// same mechanism in the batched path, which it was never carried to.
pub(crate) fn read_outcome(
    response: InternalServiceResponse,
    key: &str,
) -> Result<Option<Vec<u8>>, BackendError<WrappedMessage>> {
    match response {
        InternalServiceResponse::LocalDBGetKVSuccess(success) => Ok(Some(success.value)),
        InternalServiceResponse::LocalDBGetKVFailure(failure)
            if failure.message == KEY_NOT_FOUND =>
        {
            Ok(None)
        }
        InternalServiceResponse::LocalDBGetKVFailure(failure) => Err(BackendError::StorageError(
            format!("Failed to read key={key}: {}", failure.message),
        )),
        other => Err(BackendError::StorageError(format!(
            "Unexpected response reading key={key}: {other:?}"
        ))),
    }
}

#[cfg(test)]
mod response_classification {
    //! What counts as a stored write, and what counts as an absent key.
    //!
    //! These two questions were answered four different ways across five call
    //! sites in this file, and two of the answers were wrong in the direction
    //! that loses data silently. Testing the decisions rather than the sockets
    //! is why there is nothing mocked here: the functions are pure, so the whole
    //! space of responses can be walked with real values.
    use super::*;
    use citadel_internal_service_types::{
        LocalDBGetKVFailure, LocalDBGetKVSuccess, LocalDBSetKVFailure, LocalDBSetKVSuccess,
    };

    fn set_ok() -> InternalServiceResponse {
        InternalServiceResponse::LocalDBSetKVSuccess(LocalDBSetKVSuccess {
            cid: 1,
            peer_cid: None,
            key: "k".into(),
            request_id: None,
        })
    }

    fn set_failed(message: &str) -> InternalServiceResponse {
        InternalServiceResponse::LocalDBSetKVFailure(LocalDBSetKVFailure {
            cid: 1,
            peer_cid: None,
            message: message.into(),
            request_id: None,
        })
    }

    fn get_ok(value: &[u8]) -> InternalServiceResponse {
        InternalServiceResponse::LocalDBGetKVSuccess(LocalDBGetKVSuccess {
            cid: 1,
            peer_cid: None,
            key: "k".into(),
            value: value.to_vec(),
            request_id: None,
        })
    }

    fn get_failed(message: &str) -> InternalServiceResponse {
        InternalServiceResponse::LocalDBGetKVFailure(LocalDBGetKVFailure {
            cid: 1,
            peer_cid: None,
            message: message.into(),
            request_id: None,
        })
    }

    #[test]
    fn an_acknowledged_write_is_a_write() {
        assert!(write_outcome(Some(set_ok()), "the outbound map").is_ok());
    }

    #[test]
    fn a_refused_write_is_not_a_write() {
        // The whole finding. `.is_some()` said yes to every one of these, so the
        // sender saw a message as sent that nothing would ever retransmit.
        for message in [
            "Backend error",
            "propose_target failed",
            "This request is not permitted for this session",
        ] {
            let outcome = write_outcome(Some(set_failed(message)), "the outbound map");
            assert!(
                outcome.is_err(),
                "a LocalDBSetKVFailure({message:?}) must not report a stored write"
            );
        }
    }

    #[test]
    fn a_write_answered_by_the_wrong_variant_is_not_a_write() {
        // A response addressed to this request that is not a set-KV answer at all
        // is a protocol confusion, not a success.
        assert!(write_outcome(Some(get_ok(b"x")), "the outbound map").is_err());
    }

    #[test]
    fn an_unanswered_write_is_not_a_write() {
        assert!(write_outcome(None, "the outbound map").is_err());
    }

    #[test]
    fn a_stored_value_reads_back() {
        assert_eq!(
            read_outcome(get_ok(b"hello"), "k").unwrap(),
            Some(b"hello".to_vec())
        );
    }

    #[test]
    fn a_missing_key_is_absent_not_an_error() {
        // The one case that legitimately maps to None -- and it is keyed to the
        // constant the agent writes, not to a string retyped here.
        assert_eq!(read_outcome(get_failed(KEY_NOT_FOUND), "k").unwrap(), None);
    }

    #[test]
    fn a_failed_read_is_not_an_absent_key() {
        // `_ => None` made these indistinguishable from the case above, which is
        // how a backend error became an empty delivery frontier.
        let outcome = read_outcome(get_failed("Backend error: disk failure"), "k");
        assert!(
            outcome.is_err(),
            "a failed read must not read as an absent key"
        );
    }

    #[test]
    fn the_agent_and_this_module_agree_on_what_missing_means() {
        // The two sides of KEY_NOT_FOUND live in different crates and are compared
        // with `==`. If the agent reworded its message, every genuine miss would
        // become a hard error; this asserts the exact value both sides share.
        assert_eq!(KEY_NOT_FOUND, "Key not found");
    }
}

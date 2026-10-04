//! Refusals: what a refused request is answered with, and when it is refused.
use super::{
    gate_decision, refusal_response, requires_owned_session, GateDecision, HandledRequestResult,
    REFUSED,
};
use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
use uuid::Uuid;

/// Every gated variant, with the ids the response must echo back.
pub(super) fn gated_requests(request_id: Uuid, cid: u64) -> Vec<InternalServiceRequest> {
    vec![
        InternalServiceRequest::LocalDBSetKV {
            request_id,
            cid,
            peer_cid: None,
            key: "k".into(),
            value: vec![],
        },
        InternalServiceRequest::LocalDBDeleteKV {
            request_id,
            cid,
            peer_cid: None,
            key: "k".into(),
        },
        InternalServiceRequest::LocalDBClearAllKV {
            request_id,
            cid,
            peer_cid: None,
        },
        InternalServiceRequest::LocalDBGetAllKV {
            request_id,
            cid,
            peer_cid: None,
        },
    ]
}

/// Requests the gate refuses only when the session belongs to ANOTHER
/// connection — as an orphaned session does, which is the whole Previous
/// Sessions flow.
///
/// `gated_requests` above holds the four LocalDB variants, which are refused
/// for an unmapped session as well. These two are not, so they need their
/// own list — and because they were in neither, every test here proved the
/// builder answers the LocalDB variants and nothing at all about the
/// destructive pair the comment on `handle` names by name: "Deregister,
/// Disconnect, Message, SendFile ... are gated now".
fn refused_when_owned_elsewhere(request_id: Uuid, cid: u64) -> Vec<InternalServiceRequest> {
    vec![
        InternalServiceRequest::Disconnect { request_id, cid },
        InternalServiceRequest::Deregister { request_id, cid },
    ]
}

/// Signing out of a session another connection holds must be ANSWERED.
///
/// The gate refuses `Some(owner) if owner != caller` whatever the request
/// is, and an orphaned session's owner is the connection that opened it —
/// so signing one out from a new tab, which is the entire Previous Sessions
/// flow, lands here. `refusal_response` fell through to `_ => return None`
/// for both of these, which sends nothing at all.
///
/// Measured in CI: `Failed to disconnect: Error: Disconnect request timed
/// out` after the full thirty-second budget, over a sign-out modal that
/// spun for all of it, while a `Refusing Disconnect for session …` line sat
/// in the server log where no user can see it. The session was still there
/// afterwards, and nothing said why.
///
/// Not guesswork, which is what the doc above gives as the reason for
/// dropping everything else: both have a failure variant the client already
/// matches on, by request id.
#[test]
fn a_refused_sign_out_is_answered_rather_than_dropped() {
    let request_id = Uuid::new_v4();
    let mine = Uuid::new_v4();

    for command in refused_when_owned_elsewhere(request_id, 7) {
        assert!(
            matches!(
                gate_decision(&command, Some(false)),
                GateDecision::Refuse { .. }
            ),
            "{command:?}"
        );
        let result = refusal_response(&command, mine, REFUSED)
            .unwrap_or_else(|| panic!("no response for {command:?}"));
        assert_eq!(result.uuid, mine);
        let debug = format!("{:?}", result.response);
        assert!(
            debug.contains(&request_id.to_string()),
            "the caller is waiting on this request id: {debug}"
        );
        assert!(
            debug.contains("Session unavailable to this connection"),
            "every refusal says the same thing: {debug}"
        );
    }
}

/// The refusal must ANSWER, carrying the request id the caller is waiting on.
///
/// Refusing by `return None` sends nothing, and the browser then waits out
/// its own five-second timeout with no idea why. A response without the
/// request id is no better: nothing correlates it to the pending call.
#[test]
fn a_refused_local_db_request_is_answered_with_its_own_request_id() {
    let request_id = Uuid::new_v4();
    let uuid = Uuid::new_v4();
    for command in gated_requests(request_id, 7) {
        let result = refusal_response(&command, uuid, REFUSED)
            .unwrap_or_else(|| panic!("no response for {command:?}"));
        assert_eq!(result.uuid, uuid);
        let echoed = match &result.response {
            InternalServiceResponse::LocalDBSetKVFailure(r) => (r.request_id, r.cid),
            InternalServiceResponse::LocalDBDeleteKVFailure(r) => (r.request_id, r.cid),
            InternalServiceResponse::LocalDBClearAllKVFailure(r) => (r.request_id, r.cid),
            InternalServiceResponse::LocalDBGetAllKVFailure(r) => (r.request_id, r.cid),
            other => panic!("wrong response shape: {other:?}"),
        };
        assert_eq!(echoed, (Some(request_id), 7));
    }
}

/// Both refusal branches must be indistinguishable.
///
/// "No such session" and "not yours" are answered identically on purpose:
/// answering at all is only safe while it tells a prober nothing a timeout
/// did not already tell them.
#[test]
fn every_refusal_says_the_same_thing() {
    let messages: Vec<String> = gated_requests(Uuid::new_v4(), 7)
        .iter()
        .map(|command| {
            match refusal_response(command, Uuid::new_v4(), REFUSED)
                .unwrap()
                .response
            {
                InternalServiceResponse::LocalDBSetKVFailure(r) => r.message,
                InternalServiceResponse::LocalDBDeleteKVFailure(r) => r.message,
                InternalServiceResponse::LocalDBClearAllKVFailure(r) => r.message,
                InternalServiceResponse::LocalDBGetAllKVFailure(r) => r.message,
                other => panic!("wrong response shape: {other:?}"),
            }
        })
        .collect();
    assert_eq!(
        messages
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        1
    );
    // And it must not name which branch refused.
    assert!(!messages[0].to_lowercase().contains("own"));
}

/// A refused request is answered when it has somewhere to say so.
///
/// This asserted that `LocalDBGetKV` gets NO response, quoting the rule that
/// "inventing a response shape would be guesswork". That rule is right, and
/// it did not apply: `LocalDBGetKVFailure` is a real variant, the handler
/// already builds one when `propose_target` fails, and the client matches it
/// by request id. Nothing was being invented.
///
/// The false premise mattered. Being silent is exactly why the variant could
/// not be gated -- gating it would have refused reads into nothing and left
/// the browser waiting out its own timeout -- and not being gated is what let
/// any connection read a disconnected account's store.
///
/// `GroupListGroupsFor` genuinely has no failure variant and stays silent.
#[test]
fn a_refused_read_is_answered_rather_than_dropped() {
    let read = InternalServiceRequest::LocalDBGetKV {
        request_id: Uuid::new_v4(),
        cid: 1,
        peer_cid: None,
        key: "k".into(),
    };
    let answer = refusal_response(&read, Uuid::new_v4(), REFUSED);
    assert!(
        matches!(
            answer,
            Some(HandledRequestResult {
                response: InternalServiceResponse::LocalDBGetKVFailure(_),
                ..
            })
        ),
        "a refused read must answer with its own failure variant"
    );

    // Still silent, and for the reason that survives scrutiny.
    let groups = InternalServiceRequest::GroupListGroupsFor {
        request_id: Uuid::new_v4(),
        cid: 1,
        peer_cid: Some(2),
    };
    assert!(refusal_response(&groups, Uuid::new_v4(), REFUSED).is_none());
    // Whatever `requires_owned_session` covers, `refusal_response` must
    // answer -- otherwise a variant added to the gate silently hangs again.
    for command in gated_requests(Uuid::new_v4(), 1) {
        assert!(requires_owned_session(&command));
        assert!(refusal_response(&command, Uuid::new_v4(), REFUSED).is_some());
    }
}

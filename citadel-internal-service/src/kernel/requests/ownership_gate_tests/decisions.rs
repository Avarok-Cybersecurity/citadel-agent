//! Decisions: which sessions refuse, and which proceed.
use super::refusals::gated_requests;
use super::{gate_decision, refusal_response, GateDecision, REFUSED};
use citadel_internal_service_types::InternalServiceRequest;
use uuid::Uuid;

/// The DECISION, not just the response builder.
///
/// Restoring the silent `return None` at the call site used to pass every
/// test here, because they only exercised the thing that builds a refusal
/// and nothing obliged the gate to build one.
#[test]
fn a_refused_request_never_decides_to_proceed() {
    let mine = Uuid::new_v4();
    for command in gated_requests(Uuid::new_v4(), 7) {
        // No mapped session: refused, and the refusal is answerable.
        let unmapped = gate_decision(&command, None);
        assert!(
            matches!(unmapped, GateDecision::Refuse { .. }),
            "{command:?}"
        );
        assert!(refusal_response(&command, mine, REFUSED).is_some());
        // Mapped to somebody else: refused too.
        assert!(matches!(
            gate_decision(&command, Some(false)),
            GateDecision::Refuse { .. }
        ));
        // Mapped to the caller: allowed through.
        assert_eq!(gate_decision(&command, Some(true)), GateDecision::Proceed);
    }
}

/// The agent's own scratch space is not somebody else's session.
///
/// CID 0 names no account. The reads already went through -- `LocalDBGetKV`
/// needs no ownership, so it reached the handler and answered like any
/// other missing key -- while the writes were refused, so the
/// auto-reconnect preference could never be saved at all.
#[test]
fn cid_zero_is_not_a_session_anyone_owns() {
    for command in gated_requests(Uuid::new_v4(), 0) {
        assert_eq!(gate_decision(&command, None), GateDecision::Proceed);
        // Not even when the map happens to hold something under 0: there is
        // no account there to protect.
        assert_eq!(gate_decision(&command, Some(false)), GateDecision::Proceed);
    }
}

/// A real session is still protected.
#[test]
fn a_real_session_is_still_refused_to_a_stranger() {
    for command in gated_requests(Uuid::new_v4(), 7) {
        assert!(matches!(
            gate_decision(&command, Some(false)),
            GateDecision::Refuse { .. }
        ));
    }
}

/// A read of an unmapped session is REFUSED.
///
/// This asserted the opposite, on the reasoning that the gate "was never
/// meant to stop the handler from reporting an unknown cid honestly". The
/// handler does report an UNKNOWN cid honestly -- `propose_target` fails and
/// it answers. But for a cid naming an account that is KNOWN and merely has
/// no live session, `propose_target` SUCCEEDS: by its own doc it checks only
/// that the cid names a locally-known account. The read then hands that
/// account's stored ILM payloads to any connection that can name the cid,
/// and a cid is a u64 that travels in peer lists and `GetSessions`
/// responses, not a secret.
///
/// So the test was pinning the hole rather than the property. The honest
/// report the old reasoning wanted is still there -- it is now a refusal
/// with a `LocalDBGetKVFailure`, which the client already matches on.
#[test]
fn an_unmapped_session_refuses_a_read() {
    let read = InternalServiceRequest::LocalDBGetKV {
        request_id: Uuid::new_v4(),
        cid: 1,
        peer_cid: None,
        key: "credentials".into(),
    };
    assert!(matches!(
        gate_decision(&read, None),
        GateDecision::Refuse { .. }
    ));
}

/// Deregistering an unmapped session is refused, and answered.
///
/// `deregister::handle` never consults the connection map: it sends
/// `DeregisterFromHypernode{cid}` for whatever cid it is given. Ungated for
/// an unmapped session, that deletes an account permanently on the word of
/// any connection -- the most irreversible operation the agent has, on the
/// least evidence.
#[test]
fn an_unmapped_session_refuses_a_deregister() {
    let dereg = InternalServiceRequest::Deregister {
        request_id: Uuid::new_v4(),
        cid: 1,
    };
    assert!(matches!(
        gate_decision(&dereg, None),
        GateDecision::Refuse { .. }
    ));
    assert!(refusal_response(&dereg, Uuid::new_v4(), REFUSED).is_some());
}

/// An owned session still reads, which is the access ILM actually needs.
///
/// The control for the two above: if the gate refused reads outright rather
/// than only unowned ones, every messenger read would fail and both tests
/// would still pass.
#[test]
fn an_owned_session_still_allows_a_read() {
    let read = InternalServiceRequest::LocalDBGetKV {
        request_id: Uuid::new_v4(),
        cid: 1,
        peer_cid: None,
        key: "inbound_messages-1".into(),
    };
    assert_eq!(gate_decision(&read, Some(true)), GateDecision::Proceed);
}

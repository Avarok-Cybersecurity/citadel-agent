//! The ownership gate in mod.rs: who may act on a session.
use super::{
    gate_decision, is_exempt_from_ownership_gate, is_ilm_key_for, refusal_response,
    requires_owned_session, GateDecision, HandledRequestResult, REFUSED,
};
use citadel_internal_service_types::InternalServiceRequest;
use uuid::Uuid;

fn get_kv(key: &str) -> InternalServiceRequest {
    get_kv_for(key, 1)
}

fn get_kv_for(key: &str, cid: u64) -> InternalServiceRequest {
    InternalServiceRequest::LocalDBGetKV {
        request_id: Uuid::new_v4(),
        cid,
        peer_cid: None,
        key: key.to_string(),
    }
}

#[test]
fn only_ilm_reads_may_name_a_session_the_connection_does_not_own() {
    assert!(is_exempt_from_ownership_gate(&get_kv_for("last_sent-1", 1)));
    // The whole variant used to be exempt, so any key rode through.
    assert!(!is_exempt_from_ownership_gate(&get_kv("credentials")));
}

#[test]
fn an_ilm_key_for_another_account_is_not_exempt() {
    // The exemption's remaining hole: the key's digits were never compared
    // to the request's own cid, so `inbound_messages-<victim>` rode through
    // and handed back that account's stored P2P payloads. A cid is not a
    // secret; it travels in peer lists and GetSessions responses.
    assert!(!is_exempt_from_ownership_gate(&get_kv_for(
        "inbound_messages-999",
        1
    )));
    assert!(is_exempt_from_ownership_gate(&get_kv_for(
        "inbound_messages-999",
        999
    )));
}

#[test]
fn the_suffix_is_compared_as_written() {
    // Compared as a string, not parsed: `007` parses to 7 and is not a key
    // ILM would ever write, and accepting it would widen the exemption for
    // nothing.
    assert!(is_ilm_key_for("last_sent-7", 7));
    assert!(!is_ilm_key_for("last_sent-007", 7));
    assert!(!is_ilm_key_for("last_sent-", 7));
    assert!(!is_ilm_key_for("last_sent-7x", 7));
    assert!(!is_ilm_key_for("credentials", 7));
}

#[test]
fn no_other_request_is_exempt() {
    let write = InternalServiceRequest::LocalDBSetKV {
        request_id: Uuid::new_v4(),
        cid: 1,
        peer_cid: None,
        key: "last_sent-123".to_string(),
        value: vec![],
    };
    // An ILM-shaped KEY must not exempt a WRITE.
    assert!(!is_exempt_from_ownership_gate(&write));
    assert!(requires_owned_session(&write));
}

#[test]
fn every_local_db_write_requires_an_owned_session() {
    let id = Uuid::new_v4();
    for command in [
        InternalServiceRequest::LocalDBSetKV {
            request_id: id,
            cid: 1,
            peer_cid: None,
            key: "k".into(),
            value: vec![],
        },
        InternalServiceRequest::LocalDBDeleteKV {
            request_id: id,
            cid: 1,
            peer_cid: None,
            key: "k".into(),
        },
        InternalServiceRequest::LocalDBClearAllKV {
            request_id: id,
            cid: 1,
            peer_cid: None,
        },
        InternalServiceRequest::LocalDBGetAllKV {
            request_id: id,
            cid: 1,
            peer_cid: None,
        },
    ] {
        assert!(
            requires_owned_session(&command),
            "an unmapped cid must not be enough for {command:?}",
        );
    }
}

#[test]
fn recognises_every_key_ilm_actually_uses() {
    for key in [
        "inbound_messages-123",
        "outbound_messages-123",
        "last_acked-123",
        "last_sent-123",
        "next_unique_id-123",
        "received_messages-123",
        "last_received_from-123",
    ] {
        assert!(
            is_ilm_key_for(key, 123),
            "{key} is a key ILM reads on the happy path"
        );
    }
}

#[test]
fn refuses_anything_else() {
    for key in [
        // The whole point: an arbitrary key used to ride the exemption.
        "credentials",
        "session-token",
        // A prefix match alone is not enough — the tail must be a cid.
        "last_sent-../credentials",
        "inbound_messages-abc",
        "inbound_messages-",
        // And a lookalike must not pass.
        "not_last_sent-123",
    ] {
        assert!(
            !is_ilm_key_for(key, 123),
            "{key} must not ride the ILM exemption"
        );
    }
}

#[cfg(test)]
mod decisions;
#[cfg(test)]
mod refusals;

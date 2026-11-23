//! `citadel.session` (CW-6, citadel-session-abi.md §1): the op rows and the wire forms, asserted
//! literally against the contract, since the host is written from them in another language.

use citadel_app_sdk_types::calls::session::{
    LIMIT_SESSION_BYTES, LIMIT_SESSION_RATE, MAX_SESSION_SERVERS, SESSION_MAX_MSGS_PER_S_CEILING,
    SESSION_MAX_MSG_BYTES_CEILING,
};
use citadel_app_sdk_types::calls::*;
use citadel_app_sdk_types::ops::SessionOps;
use citadel_app_sdk_types::{
    decode, encode, AppEvent, Bytes, Capability, Group, HostCall, HostOp, OPS,
};
use serde_json::json;

fn json<T: serde::Serialize>(v: &T) -> serde_json::Value {
    serde_json::to_value(v).unwrap()
}

#[test]
fn the_session_group_is_18_and_every_op_is_admitted_by_citadel_session_alone() {
    assert_eq!(Group::Session.index(), 18);
    assert_eq!(Group::from_index(18), Some(Group::Session));
    assert_eq!(Group::from_index(17), None, "17 is reserved for identity");
    assert_eq!(Group::from_import_name("session"), Some(Group::Session));
    let rows: Vec<(&str, i32, Option<&[Capability]>)> = OPS
        .iter()
        .filter(|o| o.group == Group::Session)
        .map(|o| (o.name, o.op, o.granted_by))
        .collect();
    let session: &[Capability] = &[Capability::CitadelSession];
    assert_eq!(
        rows,
        [
            ("SessionOpen", 1, Some(session)),
            ("SessionSend", 2, Some(session)),
            ("SessionClose", 3, Some(session)),
        ]
    );
    assert_eq!((SessionOpen::GROUP, SessionOpen::OP), (Group::Session, 1));
    assert_eq!((SessionSend::GROUP, SessionSend::OP), (Group::Session, 2));
    assert_eq!((SessionClose::GROUP, SessionClose::OP), (Group::Session, 3));
}

#[test]
fn citadel_session_is_named_and_opens_the_session_group() {
    assert_eq!(
        Capability::from_name("citadel.session"),
        Some(Capability::CitadelSession)
    );
    assert_eq!(Capability::CitadelSession.name(), "citadel.session");
    assert_eq!(Capability::CitadelSession.group(), Group::Session);
    assert!(!Capability::CitadelSession.is_intrinsic());
    let wire: String = decode(&encode(&Capability::CitadelSession)).unwrap();
    assert_eq!(wire, "citadel.session");
}

#[test]
fn a_session_call_round_trips_through_group_op_and_payload() {
    let call = HostCall::Session(SessionOps::SessionSend(SessionSend {
        session: 7,
        body: Bytes(vec![1, 2]),
    }));
    let (group, op, bytes) = (call.group(), call.op(), call.encode_args());
    assert_eq!((group, op), (Group::Session, 2));
    assert_eq!(bytes, b"\xa2\x67session\x07\x64body\x42\x01\x02");
    assert_eq!(HostCall::decode(group, op, &bytes).unwrap(), call);
    assert_eq!(call.granted_by(), &[Capability::CitadelSession]);
}

#[test]
fn the_session_structs_have_the_contracts_fields() {
    let open = SessionOpen {
        server: "synchronized.work".into(),
    };
    assert_eq!(json(&open), json!({"server": "synchronized.work"}));
    assert_eq!(json(&SessionClose { session: 3 }), json!({"session": 3}));
    let state = SessionState {
        session: 3,
        cid: 42,
        state: SessionStatus::Connected,
    };
    assert_eq!(
        json(&state),
        json!({"session": 3, "cid": 42, "state": "connected"})
    );
    assert_eq!(decode::<SessionState>(&encode(&state)).unwrap(), state);
}

#[test]
fn session_status_is_snake_case_and_closed_carries_its_reason() {
    assert_eq!(encode(&SessionStatus::Connecting), b"\x6aconnecting");
    assert_eq!(encode(&SessionStatus::Connected), b"\x69connected");
    let closed = SessionStatus::Closed {
        reason: "gone".into(),
    };
    assert_eq!(json(&closed), json!({"closed": {"reason": "gone"}}));
    assert_eq!(
        encode(&closed),
        b"\xa1\x66closed\xa1\x66reason\x64gone".as_slice()
    );
    assert_eq!(decode::<SessionStatus>(&encode(&closed)).unwrap(), closed);
}

#[test]
fn a_session_event_is_tagged_session_with_its_number_and_snake_case_event() {
    let message = AppEvent::Session {
        session: 1,
        event: SessionEvent::Message {
            body: Bytes(vec![9]),
        },
    };
    assert_eq!(
        encode(&message),
        b"\xa1\x67session\xa2\x67session\x01\x65event\xa1\x67message\xa1\x64body\x41\x09"
            .as_slice()
    );
    let state = AppEvent::Session {
        session: 1,
        event: SessionEvent::State {
            state: SessionStatus::Closed { reason: "r".into() },
        },
    };
    assert_eq!(
        json(&state),
        json!({"session": {"session": 1, "event": {"state": {"state": {"closed": {"reason": "r"}}}}}})
    );
    for e in [message, state] {
        assert_eq!(decode::<AppEvent>(&encode(&e)).unwrap(), e);
    }
}

#[test]
fn the_session_limits_are_the_contracts() {
    assert_eq!(MAX_SESSION_SERVERS, 2);
    assert_eq!(SESSION_MAX_MSG_BYTES_CEILING, 1_048_576);
    assert_eq!(SESSION_MAX_MSGS_PER_S_CEILING, 64);
    assert_ne!(LIMIT_SESSION_BYTES, LIMIT_SESSION_RATE);
}

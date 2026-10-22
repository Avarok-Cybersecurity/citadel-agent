//! The relay's boundary: what is sent, to whom, and what is dropped.

use super::envelope::{read, wrap, Inbound};
use super::*;
use crate::kernel::conversations::cbor::Value;
use crate::kernel::conversations::command::{self, Inbound as Conversation};
use citadel_ihttp_types::{encode, IhttpFrame, IhttpFrameKind, IhttpMethod, MAX_FRAME_BYTES};

fn request_head() -> IhttpFrame {
    IhttpFrame {
        v: 1,
        stream: 9,
        kind: IhttpFrameKind::RequestHead {
            method: IhttpMethod::Get,
            path: "/cards".into(),
            query: None,
            headers: vec![],
            body_len: Some(0),
            app_id: "com.acme.kanban".into(),
            scope_key: "workspace".into(),
        },
    }
}

fn end_frame() -> Vec<u8> {
    encode(&IhttpFrame {
        v: 1,
        stream: 9,
        kind: IhttpFrameKind::End,
    })
    .expect("valid")
}

/// `bytes` with the first occurrence of `from` replaced by `to` (same length).
fn patched(bytes: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let at = bytes
        .windows(from.len())
        .position(|w| w == from)
        .expect("pattern present");
    let mut out = bytes.to_vec();
    out[at..at + to.len()].copy_from_slice(to);
    out
}

fn in_envelope(payload: Vec<u8>) -> Vec<u8> {
    Value::object(vec![
        ("type", Value::text(citadel_ihttp_types::ENVELOPE_KIND)),
        ("payload", Value::Bytes(payload)),
    ])
    .encode()
}

#[test]
fn a_frame_round_trips_through_its_envelope() {
    let frame = request_head();
    let wrapped = wrap(&frame).expect("wraps");
    assert!(envelope::is_envelope(&wrapped));
    assert_eq!(read(&wrapped), Inbound::Frame(encode(&frame).unwrap()));
}

#[test]
fn ordinary_traffic_is_not_mistaken_for_an_envelope() {
    let chat = Value::object(vec![
        ("type", Value::text("MessagingLayerCommand")),
        ("payload", Value::object(vec![])),
    ])
    .encode();
    for other in [chat.as_slice(), b"plain text", b"", &[0xb9, 0x00]] {
        assert_eq!(read(other), Inbound::Other);
    }
}

#[test]
fn malformed_frames_are_dropped_at_the_boundary() {
    let end = end_frame();
    let wrong_version = patched(&end, &[0x61, b'v', 0x01], &[0x61, b'v', 0x02]);
    let unknown_kind = patched(&end, b"End", b"Foo");
    let mut trailing = end.clone();
    trailing.push(0x00);
    let oversize = in_envelope(vec![0; MAX_FRAME_BYTES + 1]);
    let not_bytes = Value::object(vec![
        ("type", Value::text(citadel_ihttp_types::ENVELOPE_KIND)),
        ("payload", Value::text("GET / HTTP/1.1")),
    ])
    .encode();
    for (what, bytes) in [
        ("wrong version", in_envelope(wrong_version.clone())),
        ("unknown kind", in_envelope(unknown_kind.clone())),
        ("trailing bytes", in_envelope(trailing)),
        ("oversize", oversize),
        ("payload not bytes", not_bytes),
        ("garbage", in_envelope(vec![0xff, 0x00, 0x13])),
    ] {
        assert!(
            matches!(read(&bytes), Inbound::Malformed(_)),
            "{what}: {:?}",
            read(&bytes)
        );
    }
    // The same frames from a window are refused before anything is sent.
    let counters = IhttpCounters::default();
    for bad in [wrong_version, unknown_kind, vec![0x01]] {
        assert_eq!(
            outbound(&bad, true, &counters),
            Err(IhttpSendRefusal::Malformed)
        );
    }
    assert_eq!(counters.read().malformed_out, 3);
    assert_eq!(counters.read().sent, 0);
}

#[test]
fn a_peer_that_did_not_advertise_internal_http_is_never_sent_a_frame() {
    let counters = IhttpCounters::default();
    assert_eq!(
        outbound(&end_frame(), false, &counters),
        Err(IhttpSendRefusal::NotAdvertised)
    );
    assert_eq!(counters.read().not_advertised, 1);
    let sent = outbound(&end_frame(), true, &counters).expect("advertised");
    assert!(envelope::is_envelope(&sent));
    assert_eq!(counters.read().sent, 1);
}

#[test]
fn only_windows_that_declared_ihttp_receive_a_frame() {
    let (modern, legacy, undeclared) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let caps = HashMap::from([
        (
            modern,
            ClientCapabilities {
                agent_ilm: true,
                ihttp: true,
            },
        ),
        (
            legacy,
            ClientCapabilities {
                agent_ilm: true,
                ihttp: false,
            },
        ),
    ]);
    assert_eq!(
        receivers(&[modern, legacy, undeclared], &caps),
        vec![modern]
    );
    assert!(receivers(&[legacy, undeclared], &caps).is_empty());
}

/// What an agent that predates internal HTTP does with an envelope: its conversation store
/// reads a command of a type it does not know and hands it to a window as such, never as a
/// stored chat message, a notice or a call. (The web UI's dispatcher then ignores the type:
/// `handleP2PCommand`'s default branch.)
#[test]
fn a_legacy_receiver_treats_an_envelope_as_an_unknown_command_not_as_chat() {
    let wrapped = wrap(&request_head()).expect("wraps");
    assert_eq!(command::read(&wrapped), Some(Conversation::ForAWindow));
    assert!(!command::is_call_invite(&wrapped));
    assert!(smuggled(&wrapped));
}

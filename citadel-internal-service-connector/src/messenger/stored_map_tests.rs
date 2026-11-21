//! The stored-queue format: only the current envelope is read.

use super::*;
use citadel_internal_service_types::MessageSendSuccess;

const OWNER: u64 = 5;
const PEER: u64 = 9;

fn wrapped(id: u64, contents: InternalServicePayload) -> WrappedMessage {
    WrappedMessage {
        source_id: OWNER,
        destination_id: PEER,
        message_id: id,
        contents,
    }
}

fn outbound(id: u64) -> WrappedMessage {
    wrapped(
        id,
        InternalServicePayload::Request(InternalServiceRequest::Message {
            request_id: Uuid::from_u128(id as u128),
            message: vec![1, 2, 3, id as u8],
            cid: OWNER,
            peer_cid: Some(PEER),
            security_level: citadel_internal_service_types::SecurityLevel::Custom(10),
        }),
    )
}

fn inbound(id: u64) -> WrappedMessage {
    wrapped(
        id,
        InternalServicePayload::Response(InternalServiceResponse::MessageNotification(
            MessageNotification {
                message: vec![9, id as u8],
                cid: OWNER,
                peer_cid: PEER,
                request_id: None,
            },
        )),
    )
}

fn one(m: WrappedMessage) -> State {
    State::from([(PEER, HashMap::from([(m.message_id, m)]))])
}

/// Each message as bytes, so two states compare without `PartialEq`.
fn fingerprint(state: &State) -> Vec<(u64, u64, Vec<u8>)> {
    let mut out: Vec<_> = state
        .iter()
        .flat_map(|(p, ms)| {
            ms.iter()
                .map(|(id, m)| (*p, *id, bincode2::serialize(m).unwrap()))
        })
        .collect();
    out.sort();
    out
}

/// A queue with no envelope, even one in today's in-memory layout and holding
/// only this account's messages, was written before the versioned format: it is
/// refused, not adopted.
#[test]
fn a_bare_queue_is_unreadable_even_in_todays_layout() {
    let state = State::from([(PEER, HashMap::from([(1, outbound(1)), (2, outbound(2))]))]);
    let bare = bincode2::serialize(&state).unwrap();
    assert!(matches!(decode(&bare), Decoded::Unreadable(_)));
}

#[test]
fn both_queued_payloads_round_trip() {
    let state = State::from([(PEER, HashMap::from([(1, outbound(1)), (2, inbound(2))]))]);
    match decode(&encode(&state).unwrap()) {
        Decoded::Current(got) => assert_eq!(fingerprint(&got), fingerprint(&state)),
        other => panic!("{other:?}"),
    }
}

/// Anything else could only be stored positionally: refused, not written.
#[test]
fn any_other_payload_is_refused_on_write() {
    let other = wrapped(
        3,
        InternalServicePayload::Response(InternalServiceResponse::MessageSendSuccess(
            MessageSendSuccess {
                cid: OWNER,
                peer_cid: Some(PEER),
                request_id: None,
            },
        )),
    );
    assert!(encode(&one(other)).is_err());
}

#[test]
fn the_tags_are_pinned() {
    // envelope (5) + map len, peer, map len, id (32) + three ids (24) = 61.
    const TAG_AT: usize = 61;
    let tag = |m: WrappedMessage| {
        let bytes = encode(&one(m)).unwrap();
        assert_eq!(&bytes[..5], b"ILMQ\x01");
        u32::from_le_bytes(bytes[TAG_AT..TAG_AT + 4].try_into().unwrap())
    };
    assert_eq!(tag(outbound(1)), 0);
    assert_eq!(tag(inbound(1)), 1);
}

#[test]
fn a_newer_format_or_a_torn_blob_is_unreadable() {
    let mut newer = encode(&one(outbound(1))).unwrap();
    newer[4] = FORMAT_V1 + 1;
    assert!(matches!(
        decode(&newer),
        Decoded::Unreadable(_)
    ));
    let current = encode(&one(outbound(1))).unwrap();
    let torn = &current[..current.len() - 1];
    assert!(matches!(
        decode(torn),
        Decoded::Unreadable(_)
    ));
}

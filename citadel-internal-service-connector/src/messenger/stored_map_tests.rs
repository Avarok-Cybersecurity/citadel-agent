//! The stored-queue format, pinned against bytes an older agent really wrote.
//!
//! `stored_map_fixtures/outbound-agent-0.11.0-{1,2}.bin` are outbound queues
//! serialised with agent 0.11.0's types (citadel-internal-service 11a0ca92),
//! for session 5 sending to peer 9. They are the shape of the owner's queue
//! after the 0.11.0 -> 0.13.0 upgrade.

use super::*;
use citadel_internal_service_types::MessageSendSuccess;

const OWNER: u64 = 5;
const PEER: u64 = 9;
const OLD_ONE: &[u8] = include_bytes!("stored_map_fixtures/outbound-agent-0.11.0-1.bin");
const OLD_TWO: &[u8] = include_bytes!("stored_map_fixtures/outbound-agent-0.11.0-2.bin");

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

#[test]
fn the_owners_error_is_this_layout_shift() {
    let err = bincode2::deserialize::<State>(OLD_TWO)
        .unwrap_err()
        .to_string();
    assert!(err.contains("expected variant index 0 <= i < 2"), "{err}");
}

#[test]
fn a_one_message_old_queue_decoded_as_the_wrong_request() {
    let state: State = bincode2::deserialize(OLD_ONE).unwrap();
    let m = state[&PEER].values().next().unwrap();
    assert!(
        !matches!(
            m.contents,
            InternalServicePayload::Request(InternalServiceRequest::Message { .. })
        ),
        "the 0.11.0 Message tag no longer names Message"
    );
}

#[test]
fn old_layout_queues_are_unreadable_not_misread() {
    for bytes in [OLD_ONE, OLD_TWO] {
        let decoded = decode(bytes, QueueKind::Outbound, OWNER);
        assert!(matches!(decoded, Decoded::Unreadable(_)), "{decoded:?}");
    }
}

#[test]
fn a_bare_queue_in_todays_layout_is_kept() {
    let state = State::from([(PEER, HashMap::from([(1, outbound(1)), (2, outbound(2))]))]);
    let bare = bincode2::serialize(&state).unwrap();
    match decode(&bare, QueueKind::Outbound, OWNER) {
        Decoded::LegacyAccepted(got) => assert_eq!(fingerprint(&got), fingerprint(&state)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_bare_queue_of_the_wrong_kind_or_owner_is_unreadable() {
    let bare = bincode2::serialize(&one(outbound(1))).unwrap();
    assert!(matches!(
        decode(&bare, QueueKind::Inbound, OWNER),
        Decoded::Unreadable(_)
    ));
    assert!(matches!(
        decode(&bare, QueueKind::Outbound, 77),
        Decoded::Unreadable(_)
    ));
}

#[test]
fn both_queued_payloads_round_trip() {
    let state = State::from([(PEER, HashMap::from([(1, outbound(1)), (2, inbound(2))]))]);
    match decode(&encode(&state).unwrap(), QueueKind::Outbound, OWNER) {
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
        decode(&newer, QueueKind::Outbound, OWNER),
        Decoded::Unreadable(_)
    ));
    let current = encode(&one(outbound(1))).unwrap();
    let torn = &current[..current.len() - 1];
    assert!(matches!(
        decode(torn, QueueKind::Outbound, OWNER),
        Decoded::Unreadable(_)
    ));
}

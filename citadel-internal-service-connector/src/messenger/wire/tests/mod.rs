//! Frames as this build writes and reads them.

pub(super) use super::*;
pub(super) use citadel_internal_service_types::{InternalServiceRequest, SecurityLevel};
pub(super) use intersession_layer_messaging::{CapabilityEvidence, PeerCapabilities};

pub(super) const ALICE: u64 = 0x9f3a_1c2b_7d4e_5a60;
pub(super) const BOB: u64 = 0x1234_5678_9abc_def0;
pub(super) const ID: u64 = 1_759_071_000_000_000;

pub(super) fn everything() -> PeerCapabilities {
    PeerCapabilities::from_wire(0xff, u32::MAX)
}

pub(super) fn data(contents: Vec<u8>) -> WrappedMessage {
    WrappedMessage {
        source_id: ALICE,
        destination_id: BOB,
        message_id: ID,
        contents: InternalServicePayload::Request(InternalServiceRequest::Message {
            request_id: Uuid::nil(),
            message: contents,
            cid: ALICE,
            peer_cid: Some(BOB),
            security_level: SecurityLevel::Reinforced,
        }),
    }
}

pub(super) fn frame_bytes(frame: OutboundFrame<WrappedMessage>) -> Vec<u8> {
    match to_request(frame).expect("encode") {
        InternalServiceRequest::Message { message, .. } => message,
        other => panic!("not a message request: {other:?}"),
    }
}

pub(super) fn ack() -> InternalMessage {
    InternalMessage::Ack {
        from_id: BOB,
        to_id: ALICE,
        message_id: ID,
    }
}

pub(super) fn poll() -> InternalMessage {
    InternalMessage::Poll {
        from_id: BOB,
        to_id: ALICE,
        last_received_from_peer: Some(ID),
    }
}

pub(super) type Data = (u64, u64, u64, Vec<u8>, Option<u64>);

pub(super) fn data_of(decoded: Result<Decoded, DecodeError>) -> Data {
    match decoded.expect("decode") {
        Decoded::Data {
            source,
            destination,
            message_id,
            contents,
            piggybacked_ack,
        } => (source, destination, message_id, contents, piggybacked_ack),
        other => panic!("not a data frame: {other:?}"),
    }
}

pub(super) fn json(len: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut n = 0;
    while out.len() < len {
        out.extend_from_slice(
            format!("{{\"node\":{n},\"kind\":\"room\",\"title\":\"Room {n}\"}},").as_bytes(),
        );
        n += 1;
    }
    out
}

mod control_frames;
mod extended_frames;
mod variant_indices;

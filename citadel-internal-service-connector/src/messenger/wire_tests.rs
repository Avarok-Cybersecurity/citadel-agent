//! Frames are what two agents on two machines exchange, so their bytes are a
//! compatibility contract, not an implementation detail. The golden bytes
//! below are written out by hand from bincode2's layout (u32 variant tag,
//! little-endian u64s, u64 length prefixes) rather than produced by the code
//! under test, so a reordered variant or a changed encoding cannot pass.
use super::*;
use citadel_internal_service_types::MessageNotification;

fn outgoing(request_id: Uuid) -> InternalMessage {
    Payload::Message(WrappedMessage {
        source_id: 7,
        destination_id: 9,
        message_id: 100,
        contents: InternalServicePayload::Request(InternalServiceRequest::Message {
            request_id,
            message: b"hello".to_vec(),
            cid: 7,
            peer_cid: Some(9),
            security_level: SecurityLevel::High,
        }),
    })
}

fn le(n: u64) -> [u8; 8] {
    n.to_le_bytes()
}

#[test]
fn a_message_frame_has_the_bytes_peers_expect() {
    let request_id = Uuid::new_v4();
    let frame = encode_outbound(outgoing(request_id)).unwrap();

    let mut golden = vec![0u8, 0, 0, 0]; // WireWrapper::Message
    golden.extend(le(7)); // source
    golden.extend(le(9)); // destination
    golden.extend(le(100)); // message_id
    golden.extend(le(5)); // contents length
    golden.extend(b"hello");
    assert_eq!(frame.bytes, golden);

    // The request around it is the original request, body replaced.
    assert_eq!(frame.request_id, request_id);
    assert_eq!((frame.cid, frame.peer_cid), (7, Some(9)));
    assert!(matches!(frame.security_level, SecurityLevel::High));
}

#[test]
fn an_ack_frame_is_an_aux_signal_to_its_destination() {
    let ack: InternalMessage = Payload::Ack {
        from_id: 9,
        to_id: 7,
        message_id: 100,
    };
    let frame = encode_outbound(ack).unwrap();

    assert_eq!(&frame.bytes[..4], &[1, 0, 0, 0], "WireWrapper::ISMAux");
    assert_eq!(
        frame.bytes[4..],
        bincode2::serialize(&Payload::<WrappedMessage>::Ack {
            from_id: 9,
            to_id: 7,
            message_id: 100
        })
        .unwrap()[..]
    );
    assert_eq!((frame.cid, frame.peer_cid), (9, Some(7)));
    assert!(matches!(frame.security_level, SecurityLevel::Standard));
}

#[test]
fn a_message_that_is_not_a_request_is_refused() {
    let not_a_request: InternalMessage = Payload::Message(WrappedMessage {
        source_id: 7,
        destination_id: 9,
        message_id: 1,
        contents: InternalServicePayload::Response(InternalServiceResponse::MessageNotification(
            MessageNotification {
                message: vec![],
                cid: 7,
                peer_cid: 9,
                request_id: None,
            },
        )),
    });
    assert!(matches!(
        encode_outbound(not_a_request),
        Err(FrameError::NotAMessageRequest(_))
    ));
}

#[test]
fn what_one_side_encodes_the_other_decodes() {
    let frame = encode_outbound(outgoing(Uuid::new_v4())).unwrap();
    let arrived = MessageNotification {
        message: frame.bytes,
        cid: 9,
        peer_cid: 7,
        request_id: None,
    };
    let Ok(InboundFrame::Message { payload, unwrapped }) = decode_inbound(arrived) else {
        panic!("a message frame did not decode as one");
    };
    assert_eq!(unwrapped.message, b"hello");
    assert_eq!((unwrapped.cid, unwrapped.peer_cid), (9, 7));
    let Payload::Message(message) = payload else {
        panic!("not a message payload");
    };
    assert_eq!(
        (
            message.source_id,
            message.destination_id,
            message.message_id
        ),
        (7, 9, 100)
    );

    let poll: InternalMessage = Payload::Poll {
        from_id: 7,
        to_id: 9,
        last_received_from_peer: Some(4),
    };
    let arrived = MessageNotification {
        message: encode_outbound(poll).unwrap().bytes,
        cid: 9,
        peer_cid: 7,
        request_id: None,
    };
    assert!(matches!(
        decode_inbound(arrived),
        Ok(InboundFrame::Signal(Payload::Poll {
            from_id: 7,
            to_id: 9,
            last_received_from_peer: Some(4)
        }))
    ));
}

#[test]
fn a_raw_message_is_handed_back_untouched() {
    let raw = MessageNotification {
        message: vec![0xff; 3],
        cid: 9,
        peer_cid: 7,
        request_id: None,
    };
    let Err((_, back)) = decode_inbound(raw) else {
        panic!("three bytes decoded as an ILM frame");
    };
    assert_eq!(back.message, vec![0xff; 3]);
}

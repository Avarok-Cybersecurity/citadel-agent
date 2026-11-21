//! Every Ack and Poll carries its sender's advertisement; one without it is refused.

use super::*;

fn advertised(make: fn() -> InternalMessage) -> Vec<u8> {
    frame_bytes(OutboundFrame {
        payload: make(),
        extensions: FrameExtensions::Advertise(everything()),
    })
}

fn unadvertised(make: fn() -> InternalMessage) -> Vec<u8> {
    bincode2::serialize(&WireWrapper::ISMAux {
        signal: Box::new(make()),
    })
    .expect("serialize")
}

#[test]
fn an_advertised_ack_or_poll_is_read_back_with_its_advertisement() {
    for make in [ack as fn() -> InternalMessage, poll] {
        let Decoded::Control { signal, evidence } =
            decode_notification(&advertised(make)).expect("decode")
        else {
            panic!("not a control frame")
        };
        assert_eq!(evidence, CapabilityEvidence::Advertised(everything()));
        assert_eq!(format!("{signal:?}"), format!("{:?}", make()));
    }
}

#[test]
fn an_ack_or_poll_without_an_advertisement_is_refused() {
    for make in [ack as fn() -> InternalMessage, poll] {
        assert!(matches!(
            decode_notification(&unadvertised(make)),
            Err(DecodeError::Unreadable(_))
        ));
        let mut foreign = unadvertised(make);
        foreign.extend_from_slice(&[0u8; 8]);
        assert!(matches!(
            decode_notification(&foreign),
            Err(DecodeError::Unreadable(_))
        ));
    }
}

#[test]
fn an_ack_or_poll_is_never_sent_without_an_advertisement() {
    for make in [ack as fn() -> InternalMessage, poll] {
        let frame = OutboundFrame {
            payload: make(),
            extensions: FrameExtensions::None,
        };
        assert!(matches!(
            to_request(frame),
            Err(EncodeError::Unencodable(_))
        ));
    }
}

#[test]
fn a_plain_data_frame_is_still_accepted() {
    let bytes = bincode2::serialize(&WireWrapper::Message {
        source: ALICE,
        destination: BOB,
        message_id: ID,
        contents: b"uncompressed".to_vec(),
    })
    .expect("serialize");
    assert_eq!(
        data_of(decode_notification(&bytes)),
        (ALICE, BOB, ID, b"uncompressed".to_vec(), None)
    );
}

#[test]
fn raw_traffic_is_still_not_a_frame() {
    assert!(matches!(
        decode_notification(&[0xa2, 0x01, 0x02]),
        Err(DecodeError::NotAFrame(_))
    ));
}

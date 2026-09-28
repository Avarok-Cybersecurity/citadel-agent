//! New frames as a legacy peer reads them, and legacy frames as this build reads them.

use super::*;

#[test]
fn a_data_frame_without_extensions_is_byte_identical_to_the_legacy_frame() {
    let contents = json(600);
    let new = frame_bytes(OutboundFrame::legacy(Payload::Message(data(
        contents.clone(),
    ))));
    let legacy = bincode2::serialize(&Legacy::Message {
        source: ALICE,
        destination: BOB,
        message_id: ID,
        contents,
    })
    .expect("serialize");
    assert_eq!(new, legacy);
}

#[test]
fn a_control_frame_without_extensions_is_byte_identical_to_the_legacy_frame() {
    for make in [ack as fn() -> InternalMessage, poll] {
        let new = frame_bytes(OutboundFrame::legacy(make()));
        let legacy = bincode2::serialize(&Legacy::ISMAux {
            signal: Box::new(make()),
        })
        .expect("serialize");
        assert_eq!(new, legacy);
    }
}

#[test]
fn a_legacy_peer_reads_an_advertised_control_frame_as_the_plain_one() {
    for make in [ack as fn() -> InternalMessage, poll] {
        let plain = frame_bytes(OutboundFrame::legacy(make()));
        let advertised = frame_bytes(OutboundFrame {
            payload: make(),
            extensions: FrameExtensions::Advertise(everything()),
        });
        assert!(
            advertised.len() > plain.len(),
            "the advertisement is not on the wire"
        );
        assert_eq!(as_legacy_reads_it(&advertised), as_legacy_reads_it(&plain));
    }
}

#[test]
fn an_advertisement_is_read_back_and_its_absence_means_legacy() {
    let advertised = frame_bytes(OutboundFrame {
        payload: ack(),
        extensions: FrameExtensions::Advertise(everything()),
    });
    let Decoded::Control { evidence, .. } = decode_notification(&advertised).expect("decode")
    else {
        panic!("not a control frame")
    };
    assert_eq!(evidence, CapabilityEvidence::Advertised(everything()));

    let from_legacy = bincode2::serialize(&Legacy::ISMAux {
        signal: Box::new(ack()),
    })
    .expect("serialize");
    let Decoded::Control { signal, evidence } = decode_notification(&from_legacy).expect("decode")
    else {
        panic!("not a control frame")
    };
    assert_eq!(
        evidence,
        CapabilityEvidence::Advertised(PeerCapabilities::LEGACY)
    );
    assert!(matches!(
        *signal,
        InternalMessage::Ack { message_id: ID, .. }
    ));
}

#[test]
fn a_legacy_data_frame_is_still_accepted() {
    let bytes = bincode2::serialize(&Legacy::Message {
        source: ALICE,
        destination: BOB,
        message_id: ID,
        contents: b"from an old peer".to_vec(),
    })
    .expect("serialize");
    assert_eq!(
        data_of(decode_notification(&bytes)),
        (ALICE, BOB, ID, b"from an old peer".to_vec(), None)
    );
}

#[test]
fn raw_traffic_is_still_not_a_frame() {
    assert!(matches!(
        decode_notification(&[0xa2, 0x01, 0x02]),
        Err(DecodeError::NotAFrame(_))
    ));
}

//! Frames only a negotiated peer is ever sent.

use super::*;
use intersession_layer_messaging::compression::CodecSet;
use intersession_layer_messaging::{CompressionHint, CompressionPlan};

fn plan(hint: CompressionHint, codecs: &[Codec]) -> CompressionPlan {
    CompressionPlan {
        hint,
        codecs: CodecSet::of(codecs),
    }
}

#[test]
fn a_piggybacked_ack_rides_uncompressed_and_comes_back_out() {
    let bytes = frame_bytes(OutboundFrame {
        payload: Payload::Message(data(b"short reply".to_vec())),
        extensions: FrameExtensions::Negotiated {
            piggybacked_ack: Some(ID - 3),
            compression: None,
        },
    });
    assert!(
        bincode2::deserialize::<Legacy>(&bytes).is_err(),
        "a legacy peer must not be able to mistake an extended frame for a plain one"
    );
    assert_eq!(
        data_of(decode_notification(&bytes)),
        (ALICE, BOB, ID, b"short reply".to_vec(), Some(ID - 3))
    );
}

#[test]
fn compression_that_does_not_pay_falls_back_to_the_legacy_frame() {
    // Below the threshold, so the codec is never even tried.
    let contents = b"tiny".to_vec();
    let bytes = frame_bytes(OutboundFrame {
        payload: Payload::Message(data(contents.clone())),
        extensions: FrameExtensions::Negotiated {
            piggybacked_ack: None,
            compression: Some(plan(
                CompressionHint::Json,
                &[Codec::Brotli, Codec::Deflate],
            )),
        },
    });
    let legacy = bincode2::serialize(&Legacy::Message {
        source: ALICE,
        destination: BOB,
        message_id: ID,
        contents,
    })
    .expect("serialize");
    assert_eq!(bytes, legacy);
}

#[test]
fn a_frame_with_an_unknown_codec_or_dictionary_is_refused_not_forwarded() {
    let v2 = |codec: u8, dict_id: u16| {
        bincode2::serialize(&WireWrapper::MessageV2 {
            source: ALICE,
            destination: BOB,
            message_id: ID,
            codec,
            dict_id,
            piggybacked_ack: None,
            contents: vec![1, 2, 3],
        })
        .expect("serialize")
    };
    assert!(matches!(
        decode_notification(&v2(99, 0)),
        Err(DecodeError::Unreadable(_))
    ));
    assert!(matches!(
        decode_notification(&v2(0, 7)),
        Err(DecodeError::Unreadable(_))
    ));
}

#[test]
fn misplaced_extensions_are_refused() {
    assert!(to_request(OutboundFrame {
        payload: Payload::Message(data(vec![1])),
        extensions: FrameExtensions::Advertise(everything()),
    })
    .is_err());
    assert!(to_request(OutboundFrame {
        payload: ack(),
        extensions: FrameExtensions::Negotiated {
            piggybacked_ack: None,
            compression: None
        },
    })
    .is_err());
}

#[cfg(all(feature = "compression-brotli", feature = "compression-deflate"))]
mod compressed {
    use super::*;

    #[test]
    fn each_codec_round_trips_and_shrinks_the_frame() {
        let contents = json(4000);
        let plain = frame_bytes(OutboundFrame::legacy(Payload::Message(data(
            contents.clone(),
        ))));
        for codec in [
            Codec::Brotli,
            Codec::Deflate,
            #[cfg(feature = "compression-zstd")]
            Codec::Zstd,
        ] {
            let bytes = frame_bytes(OutboundFrame {
                payload: Payload::Message(data(contents.clone())),
                extensions: FrameExtensions::Negotiated {
                    piggybacked_ack: Some(7),
                    compression: Some(plan(CompressionHint::Json, &[codec])),
                },
            });
            assert!(
                bytes.len() < plain.len() / 2,
                "{codec:?}: {} vs {}",
                bytes.len(),
                plain.len()
            );
            assert_eq!(
                data_of(decode_notification(&bytes)),
                (ALICE, BOB, ID, contents.clone(), Some(7))
            );
        }
    }

    #[test]
    fn corrupt_or_explosive_contents_are_refused_without_a_panic() {
        let bomb = {
            let mut out = Vec::new();
            {
                let mut writer = brotli_writer(&mut out);
                std::io::Write::write_all(
                    &mut writer,
                    &vec![0u8; compression::MAX_DECOMPRESSED_LEN + 1],
                )
                .expect("write");
            }
            out
        };
        for contents in [bomb, vec![0xde, 0xad, 0xbe, 0xef, 0x00, 0x11]] {
            let bytes = bincode2::serialize(&WireWrapper::MessageV2 {
                source: ALICE,
                destination: BOB,
                message_id: ID,
                codec: Codec::Brotli.to_wire(),
                dict_id: 0,
                piggybacked_ack: None,
                contents,
            })
            .expect("serialize");
            assert!(matches!(
                decode_notification(&bytes),
                Err(DecodeError::Unreadable(_))
            ));
        }
    }

    fn brotli_writer(out: &mut Vec<u8>) -> impl std::io::Write + '_ {
        brotli::CompressorWriter::new(out, 4096, 4, 18)
    }
}

//! Properties over the P2P envelope reader, on inputs nobody wrote by hand: it never panics,
//! a frame it reads is canonical, and every wrapped frame reads back as itself.

use super::envelope::{self, Inbound};
use crate::kernel::conversations::cbor::Value;
use citadel_ihttp_types::{decode, encode, IhttpFrame, IhttpFrameKind, ENVELOPE_KIND};
use proptest::collection::vec;
use proptest::prelude::*;

fn frame() -> impl Strategy<Value = IhttpFrame> {
    let kind = prop_oneof![
        Just(IhttpFrameKind::End),
        (1u32..=64).prop_map(|n| IhttpFrameKind::Credit { n }),
        vec(any::<u8>(), 1..256).prop_map(|bytes| IhttpFrameKind::Body { bytes }),
    ];
    (any::<u64>(), kind).prop_map(|(stream, kind)| IhttpFrame { v: 1, stream, kind })
}

/// What a read accepts is one valid frame, re-encoded to the bytes `encode` writes.
fn read_is_canonical(bytes: &[u8]) -> Result<(), TestCaseError> {
    if let Inbound::Frame(canonical) = envelope::read(bytes) {
        let frame = decode(&canonical).map_err(|why| TestCaseError::fail(why.to_string()))?;
        prop_assert_eq!(encode(&frame).ok(), Some(canonical));
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn arbitrary_bytes_never_panic(bytes in vec(any::<u8>(), 0..512)) {
        read_is_canonical(&bytes)?;
    }

    /// Past the prefix check, where the reader starts trusting structure.
    #[test]
    fn anything_after_the_envelope_prefix_never_panics(tail in vec(any::<u8>(), 0..512)) {
        let mut bytes = Value::object(vec![("type", Value::text(ENVELOPE_KIND))]).encode();
        bytes[2] = 2;
        bytes.extend(tail);
        read_is_canonical(&bytes)?;
    }

    #[test]
    fn every_wrapped_frame_reads_back(frame in frame()) {
        let wrapped = envelope::wrap(&frame).expect("a valid frame wraps");
        prop_assert!(envelope::is_envelope(&wrapped));
        prop_assert_eq!(envelope::read(&wrapped), Inbound::Frame(encode(&frame).unwrap()));
    }

    #[test]
    fn a_damaged_envelope_never_panics(
        frame in frame(),
        at in any::<prop::sample::Index>(),
        byte in any::<u8>(),
        damage in 0u8..3,
    ) {
        let mut bytes = envelope::wrap(&frame).expect("wraps");
        let i = at.index(bytes.len());
        match damage {
            0 => bytes[i] ^= byte | 1,
            1 => bytes.truncate(i),
            _ => bytes.insert(i, byte),
        }
        read_is_canonical(&bytes)?;
    }
}

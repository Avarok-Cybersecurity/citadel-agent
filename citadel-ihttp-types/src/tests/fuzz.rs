//! Properties over the decoder, on inputs nobody wrote by hand: it never panics, whatever it
//! accepts it can write back and read again unchanged, and every frame `encode` writes it
//! reads back as the same frame.

use super::*;
use proptest::collection::vec;
use proptest::prelude::*;

fn method() -> impl Strategy<Value = IhttpMethod> {
    prop_oneof![
        Just(IhttpMethod::Get),
        Just(IhttpMethod::Post),
        Just(IhttpMethod::Put),
        Just(IhttpMethod::Delete),
        Just(IhttpMethod::Patch),
        Just(IhttpMethod::Head),
    ]
}

fn reason() -> impl Strategy<Value = CancelReason> {
    prop_oneof![
        Just(CancelReason::Caller),
        Just(CancelReason::Timeout),
        Just(CancelReason::TooLarge),
        Just(CancelReason::Busy),
        Just(CancelReason::RateLimited),
        Just(CancelReason::NoRoute),
        Just(CancelReason::Refused),
        Just(CancelReason::Failed),
        Just(CancelReason::Closed),
        Just(CancelReason::Protocol),
        Just(CancelReason::Unavailable),
    ]
}

/// Mostly well-formed names and values, sometimes not, so both sides of `validate` are hit.
fn header() -> impl Strategy<Value = IhttpHeader> {
    (
        "[a-z-]{0,12}|[A-Za-z :\r\n]{1,6}",
        "[ -~]{0,24}|[\t\r\n]{1,2}",
    )
        .prop_map(|(name, value)| IhttpHeader { name, value })
}

fn kind() -> impl Strategy<Value = IhttpFrameKind> {
    prop_oneof![
        (
            method(),
            "/[a-z0-9/._-]{0,24}|[^/][ -~]{0,8}",
            proptest::option::of("[a-z0-9=&#]{0,12}"),
            vec(header(), 0..4),
            proptest::option::of(0u64..=MAX_REQUEST_BODY_BYTES + 1),
            "com\\.acme\\.[a-z]{1,8}|[A-Z ]{0,4}",
            "node:[a-z0-9]{1,8}|",
        )
            .prop_map(
                |(method, path, query, headers, body_len, app_id, scope_key)| {
                    IhttpFrameKind::RequestHead {
                        method,
                        path,
                        query,
                        headers,
                        body_len,
                        app_id,
                        scope_key,
                    }
                }
            ),
        (
            0u16..=700,
            vec(header(), 0..4),
            proptest::option::of(0u64..=MAX_RESPONSE_BODY_BYTES + 1)
        )
            .prop_map(|(status, headers, body_len)| IhttpFrameKind::ResponseHead {
                status,
                headers,
                body_len,
            }),
        vec(any::<u8>(), 0..64).prop_map(|bytes| IhttpFrameKind::Body { bytes }),
        Just(IhttpFrameKind::End),
        reason().prop_map(|reason| IhttpFrameKind::Cancel { reason }),
        (0u32..=MAX_CREDIT_GRANT + 1).prop_map(|n| IhttpFrameKind::Credit { n }),
    ]
}

fn any_frame() -> impl Strategy<Value = IhttpFrame> {
    (
        prop_oneof![Just(IHTTP_VERSION), any::<u8>()],
        any::<u64>(),
        kind(),
    )
        .prop_map(|(v, stream, kind)| IhttpFrame { v, stream, kind })
}

/// Whatever `decode` accepts, `encode` writes, and `decode` reads back unchanged.
fn accepted_round_trips(bytes: &[u8]) -> Result<(), TestCaseError> {
    if let Ok(frame) = decode(bytes) {
        let again = encode(&frame).map_err(|why| {
            TestCaseError::fail(format!("decoded {frame:?} but encode refused it: {why}"))
        })?;
        prop_assert_eq!(decode(&again), Ok(frame));
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn arbitrary_bytes_never_panic(bytes in vec(any::<u8>(), 0..512)) {
        accepted_round_trips(&bytes)?;
    }

    #[test]
    fn encode_and_validate_agree_and_every_written_frame_reads_back(frame in any_frame()) {
        let written = encode(&frame);
        prop_assert_eq!(written.is_ok(), validate(&frame).is_ok());
        if let Ok(bytes) = written {
            prop_assert_eq!(decode(&bytes), Ok(frame));
        }
    }

    /// A hostile peer writes any frame without `encode`'s checks: `decode` reads exactly the
    /// ones `validate` admits, and nothing else.
    #[test]
    fn decode_admits_exactly_what_validate_admits(frame in any_frame()) {
        let decoded = decode(&raw(&frame));
        match validate(&frame) {
            Ok(()) => prop_assert_eq!(decoded, Ok(frame)),
            Err(why) => prop_assert_eq!(decoded, Err(why)),
        }
    }

    /// A written frame with bytes flipped, cut or added: near-valid input, where a decoder
    /// that trusts a length or a tag is likeliest to slip.
    #[test]
    fn a_damaged_frame_never_panics(
        frame in any_frame(),
        at in any::<prop::sample::Index>(),
        byte in any::<u8>(),
        damage in 0u8..3,
    ) {
        let Ok(mut bytes) = encode(&frame) else { return Ok(()) };
        let i = at.index(bytes.len());
        match damage {
            0 => bytes[i] ^= byte | 1,
            1 => bytes.truncate(i),
            _ => bytes.insert(i, byte),
        }
        accepted_round_trips(&bytes)?;
    }
}

/// The round-trip property above holds only if the strategy writes valid frames often enough.
#[test]
fn the_frame_strategy_reaches_both_sides_of_validate() {
    use proptest::strategy::ValueTree;
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let strategy = any_frame();
    let valid = (0..512)
        .filter(|_| {
            let frame = strategy.new_tree(&mut runner).expect("a frame").current();
            validate(&frame).is_ok()
        })
        .count();
    assert!((64..=448).contains(&valid), "{valid} of 512 valid");
}

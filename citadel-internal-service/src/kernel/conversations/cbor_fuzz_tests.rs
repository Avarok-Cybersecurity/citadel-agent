//! Properties over the cbor-x reader, on inputs nobody wrote by hand: it never panics, and
//! whatever it reads writes back to bytes it reads as the same value.

use super::{Value, Width};
use proptest::collection::vec;
use proptest::prelude::*;

/// Every width that can carry `n` (the minimal one or any wider), as a writer may choose.
fn width_for(n: u64) -> impl Strategy<Value = Width> {
    let fits = |w: &Width| match w {
        Width::Inline => n < 24,
        Width::One => n <= 0xff,
        Width::Two => n <= 0xffff,
        Width::Four => n <= 0xffff_ffff,
        Width::Eight => true,
    };
    let widths: Vec<Width> = [
        Width::Inline,
        Width::One,
        Width::Two,
        Width::Four,
        Width::Eight,
    ]
    .into_iter()
    .filter(fits)
    .collect();
    proptest::sample::select(widths)
}

fn value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        any::<u64>().prop_flat_map(|n| width_for(n).prop_map(move |w| Value::Uint(n, w))),
        any::<u64>().prop_flat_map(|n| width_for(n).prop_map(move |w| Value::Nint(n, w))),
        vec(any::<u8>(), 0..32).prop_map(Value::Bytes),
        ".{0,16}".prop_map(Value::Text),
        any::<bool>().prop_map(Value::Bool),
        Just(Value::Null),
        Just(Value::Undefined),
        any::<f64>().prop_map(Value::Float),
    ];
    leaf.prop_recursive(4, 64, 8, |inner| {
        prop_oneof![
            vec(inner.clone(), 0..8).prop_map(Value::Array),
            (vec((inner.clone(), inner.clone()), 0..6), 0u8..2).prop_map(|(entries, wide)| {
                let width = if wide == 1 { Width::Two } else { Width::Inline };
                let width = if entries.len() >= 24 {
                    Width::Two
                } else {
                    width
                };
                Value::Map(entries, width)
            }),
            (any::<u64>(), inner).prop_map(|(tag, v)| Value::Tag(tag, Box::new(v))),
        ]
    })
}

/// Equal, with floats compared by their bits, so NaN is equal to itself.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b))
        }
        (Value::Map(x, wx), Value::Map(y, wy)) => {
            wx == wy
                && x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|((ka, va), (kb, vb))| same(ka, kb) && same(va, vb))
        }
        (Value::Tag(tx, x), Value::Tag(ty, y)) => tx == ty && same(x, y),
        _ => a == b,
    }
}

/// Compared by bytes, so NaN (unequal to itself) still compares.
fn rewrites_stably(bytes: &[u8]) -> Result<(), TestCaseError> {
    if let Ok(read) = Value::decode(bytes) {
        let written = read.encode();
        let again = Value::decode(&written)
            .map_err(|why| TestCaseError::fail(format!("its own output is unreadable: {why:?}")))?;
        prop_assert_eq!(again.encode(), written);
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn arbitrary_bytes_never_panic(bytes in vec(any::<u8>(), 0..512)) {
        rewrites_stably(&bytes)?;
    }

    #[test]
    fn every_written_value_reads_back_as_itself(value in value()) {
        let bytes = value.encode();
        let read = Value::decode(&bytes).expect("its own output reads");
        prop_assert!(same(&read, &value), "{value:?} read back as {read:?}");
    }

    #[test]
    fn a_damaged_value_never_panics(
        value in value(),
        at in any::<prop::sample::Index>(),
        byte in any::<u8>(),
        damage in 0u8..3,
    ) {
        let mut bytes = value.encode();
        let i = at.index(bytes.len());
        match damage {
            0 => bytes[i] ^= byte | 1,
            1 => bytes.truncate(i),
            _ => bytes.insert(i, byte),
        }
        rewrites_stably(&bytes)?;
    }
}

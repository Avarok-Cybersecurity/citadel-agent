use super::*;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/p2p_commands");

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{FIXTURES}/{name}.cbor")).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Every byte cbor-x wrote reads back and writes out identically.
#[test]
fn every_fixture_round_trips_to_the_bytes_cbor_x_wrote() {
    let mut seen = 0;
    for entry in std::fs::read_dir(FIXTURES).expect("fixture dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("cbor") {
            continue;
        }
        let bytes = std::fs::read(&path).expect("read");
        let value = Value::decode(&bytes).unwrap_or_else(|e| panic!("{path:?}: {e:?}"));
        assert_eq!(value.encode(), bytes, "{path:?} re-encoded differently");
        seen += 1;
    }
    assert!(
        seen >= 15,
        "only {seen} fixtures found; the check proves little"
    );
}

/// The encoder's own rules, against values it BUILDS rather than ones it read.
#[test]
fn numbers_are_encoded_as_cbor_x_encodes_them() {
    let numbers = [
        0.0,
        23.0,
        24.0,
        255.0,
        256.0,
        65535.0,
        65536.0,
        4294967295.0,
        4294967296.0,
        -1.0,
        -24.0,
        -25.0,
        -256.0,
        -257.0,
        -2147483648.0,
        -2147483649.0,
        1.5,
        0.1,
        1790000000123.0,
        -0.0,
        1e300,
    ];
    let built = Value::Array(numbers.iter().map(|n| Value::number(*n)).collect());
    assert_eq!(built.encode(), fixture("numbers"));
}

#[test]
fn strings_use_minimal_headers_by_byte_length() {
    let strings = [
        String::new(),
        "a".repeat(23),
        "a".repeat(24),
        "a".repeat(255),
        "a".repeat(256),
        "a".repeat(65536),
        "é".repeat(100),
    ];
    let built = Value::Array(strings.into_iter().map(Value::text).collect());
    assert_eq!(built.encode(), fixture("strings"));
}

#[test]
fn bigints_are_always_the_eight_byte_form() {
    let built = Value::Array([0, 5, u64::MAX].into_iter().map(Value::bigint).collect());
    assert_eq!(built.encode(), fixture("bigints"));
}

/// A JS reader tells bigint from number by the header alone.
#[test]
fn only_the_eight_byte_form_reads_as_a_bigint() {
    assert_eq!(Value::Uint(5, Width::Eight).as_bigint(), Some(5));
    assert_eq!(Value::Uint(5, Width::Inline).as_bigint(), None);
    assert_eq!(Value::Uint(5, Width::Eight).as_number(), None);
    assert_eq!(
        Value::number(1790000000123.0).as_number(),
        Some(1790000000123.0)
    );
}

#[test]
fn malformed_input_is_an_error_not_a_panic() {
    for bad in [
        &[][..],
        &[0xb9, 0xff, 0xff], // a map claiming 65535 entries in 0 bytes
        &[0x7a, 0xff, 0xff, 0xff, 0xff], // a 4 GiB string
        &[0x9f],             // indefinite array
        &[0x63, 0xff, 0xfe, 0xfd], // invalid UTF-8
        &[0x01, 0x02],       // trailing bytes
    ] {
        assert!(Value::decode(bad).is_err(), "accepted {bad:02x?}");
    }
    let deep = vec![0x81u8; 200];
    assert!(Value::decode(&deep).is_err(), "unbounded nesting accepted");
}

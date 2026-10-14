use super::*;

fn known() -> Vec<KnownValue> {
    vec![KnownValue {
        kind: RedactionKind::Username,
        value: "alice".into(),
    }]
}

#[test]
fn one_value_has_one_placeholder_within_a_batch() {
    let salt = Salt::from_bytes([1; 32]);
    let out = redact(
        "alice then Alice then 10.0.0.1 and 10.0.0.1",
        &known(),
        &salt,
    );
    let user = placeholder(RedactionKind::Username, "alice", &salt);
    let ip = placeholder(RedactionKind::Ip, "10.0.0.1", &salt);
    assert_eq!(out, format!("{user} then {user} then {ip} and {ip}"));
}

#[test]
fn a_new_salt_gives_new_placeholders() {
    let a = redact("alice", &known(), &Salt::from_bytes([1; 32]));
    let b = redact("alice", &known(), &Salt::from_bytes([2; 32]));
    assert_ne!(a, b);
}

#[test]
fn kinds_do_not_share_placeholders() {
    let salt = Salt::from_bytes([1; 32]);
    assert_ne!(
        placeholder(RedactionKind::Username, "x", &salt)[6..],
        placeholder(RedactionKind::Host, "x", &salt)[6..]
    );
}

#[test]
fn overlapping_hits_leave_no_tail() {
    // The path rule and the long-token rule overlap; neither range may partly survive.
    let input = "read /srv/k/0123456789abcdef0123456789abcdef0123/tail now";
    let out = redact(input, &[], &Salt::from_bytes([3; 32]));
    assert!(!out.contains("tail") && !out.contains("0123"), "{out}");
    assert!(out.ends_with(" now"), "{out}");
}

#[test]
fn empty_known_values_are_ignored() {
    let blank = [KnownValue {
        kind: RedactionKind::Username,
        value: "  ".into(),
    }];
    assert_eq!(redact("a b c", &blank, &Salt::from_bytes([0; 32])), "a b c");
}

#[test]
fn cids_are_never_redacted() {
    let input = "peer 18446744073709551615 and session 42 failed";
    assert_eq!(redact(input, &known(), &Salt::from_bytes([0; 32])), input);
}

#[test]
fn cid_tokens_find_digits_inside_words() {
    let found: Vec<u64> = cid_tokens("cid_123 abc456 789").collect();
    assert_eq!(found, vec![123, 456, 789]);
}

#[test]
fn a_hit_that_starts_inside_another_and_ends_past_it_leaves_no_tail() {
    let known = [
        KnownValue {
            kind: RedactionKind::FullName,
            value: "Alice Liddell".into(),
        },
        KnownValue {
            kind: RedactionKind::Username,
            value: "liddell.home".into(),
        },
    ];
    let out = redact(
        "from Alice Liddell.home today",
        &known,
        &Salt::from_bytes([4; 32]),
    );
    assert!(!out.contains("home") && !out.contains("Liddell"), "{out}");
    assert!(
        out.starts_with("from <name:") && out.ends_with("> today"),
        "{out}"
    );
}

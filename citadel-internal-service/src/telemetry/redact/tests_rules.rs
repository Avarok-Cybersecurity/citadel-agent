//! One table per rule: every row must lose its sensitive part, and every negative row
//! (a near miss for that rule) must come through untouched.

use super::*;

fn salt() -> Salt {
    Salt::from_bytes([7; 32])
}

fn known() -> Vec<KnownValue> {
    vec![
        KnownValue {
            kind: RedactionKind::Username,
            value: "alice".into(),
        },
        KnownValue {
            kind: RedactionKind::FullName,
            value: "Alice Liddell".into(),
        },
        KnownValue {
            kind: RedactionKind::Secret,
            value: "hunter2-server-pw".into(),
        },
        KnownValue {
            kind: RedactionKind::Host,
            value: "corp-gateway".into(),
        },
    ]
}

/// Each row: (input, the sensitive substring, the placeholder label it must become).
fn assert_redacts(rows: &[(&str, &str, &str)]) {
    for (input, secret, label) in rows {
        let out = redact(input, &known(), &salt());
        assert!(!out.contains(secret), "{secret:?} survived in {out:?}");
        assert!(
            out.contains(&format!("<{label}:")),
            "no <{label}:..> in {out:?}"
        );
    }
}

fn assert_untouched(rows: &[&str]) {
    for input in rows {
        assert_eq!(redact(input, &known(), &salt()), *input, "over-redacted");
    }
}

#[test]
fn known_usernames_and_names() {
    assert_redacts(&[
        ("peer alice refused", "alice", "user"),
        ("Peer ALICE refused", "ALICE", "user"),
        ("registered 'alice'", "alice", "user"),
        ("from Alice Liddell (alice)", "Liddell", "name"),
    ]);
    assert_untouched(&["malice aforethought", "alices_cat", "Liddellish"]);
}

#[test]
fn known_secrets_and_hosts() {
    assert_redacts(&[
        (
            "connect with hunter2-server-pw failed",
            "hunter2-server-pw",
            "secret",
        ),
        ("dial corp-gateway timed out", "corp-gateway", "host"),
    ]);
    assert_untouched(&["hunter2 is not the password", "corp gateway"]);
}

#[test]
fn secret_assignments_and_bearer_tokens() {
    assert_redacts(&[
        ("password=s3cr3t!", "s3cr3t!", "secret"),
        ("token: abc.def.ghi", "abc.def.ghi", "secret"),
        ("api_key=\"k-123\"", "k-123", "secret"),
        (
            "Authorization: Bearer eyJhbGciOi.x.y",
            "eyJhbGciOi",
            "secret",
        ),
        (
            "got 0123456789abcdef0123456789abcdefXY back",
            "0123456789abcdef",
            "secret",
        ),
    ]);
    assert_untouched(&[
        "password rejected by server",
        "the token expired",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ]);
}

#[test]
fn emails() {
    assert_redacts(&[("invite to bob@example.org failed", "bob@example", "email")]);
    assert_untouched(&["send @ noon", "a@b"]);
}

#[test]
fn file_paths() {
    assert_redacts(&[
        (
            "open /Users/carol/Documents/plan failed",
            "/Users/carol",
            "path",
        ),
        ("read ~/secret/notes: denied", "secret/notes", "path"),
        ("write ./out/a failed", "out/a", "path"),
        (r"open C:\Users\dave\x.txt", "dave", "path"),
        (r"share \\nas\home\erin", "erin", "path"),
        ("saved Downloads/tax/return failed", "Downloads/tax", "path"),
        ("saved Downloads/return.pdf", "Downloads/return", "path"),
    ]);
    assert_untouched(&["TCP/UDP", "3/5 retries", "N/A", "and/or"]);
}

#[test]
fn file_names() {
    assert_redacts(&[
        ("upload of tax-return.pdf failed", "tax-return", "file"),
        ("Holiday.JPG rejected", "Holiday", "file"),
    ]);
    assert_untouched(&["e.g. this", "took 1.5s", "version 0.9.0"]);
}

#[test]
fn ip_addresses() {
    assert_redacts(&[
        ("dial 192.168.1.20:25000 failed", "192.168.1.20", "ip"),
        ("peer at ip=10.0.0.7", "10.0.0.7", "ip"),
        ("bound [2001:db8::1]:443", "2001:db8::1", "ip"),
        ("via fe80::1ff:fe23:4567:890a%en0", "fe80::1ff", "ip"),
        ("mapped ::ffff:203.0.113.9", "203.0.113.9", "ip"),
    ]);
    assert_untouched(&[
        "std::io::Error",
        "at 12:30:45",
        "version 1.2.3",
        "999.1.1.1 is not an address",
    ]);
}

#[test]
fn hostnames() {
    assert_redacts(&[
        ("resolve work.avarok.net failed", "avarok", "host"),
        ("Server.Example.COM unreachable", "Example", "host"),
    ]);
    assert_untouched(&["took 2.25ms", "v0.9.0-rc.1", "done."]);
}

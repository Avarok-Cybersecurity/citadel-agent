//! `identity-kat.json` (frozen contract, identity-api.md §1–§3): every constant, every domain
//! label and every name vector, run through the crate.
//!
//! Control: accept a second `@` in `parse_username` → the `a@b@c` vector parses and
//! `every_parse_username_vector` goes red.

mod common;

use citadel_identity::*;
use common::kat::{load, number, strings, text, variant};
use serde_json::Value;

fn kat() -> Value {
    load("identity-kat.json")
}

#[test]
fn every_constant_matches_the_contract() {
    let c = &kat()["constants"];
    let ours: [(&str, u64); 23] = [
        ("ASSERTION_VERSION", ASSERTION_VERSION.into()),
        ("BINDING_FIXED_LEN", BINDING_FIXED_LEN as u64),
        ("BINDING_VERSION", BINDING_VERSION.into()),
        ("FINGERPRINT_BYTES", FINGERPRINT_BYTES as u64),
        (
            "MAX_ASSERTION_FIELD_BYTES",
            MAX_ASSERTION_FIELD_BYTES as u64,
        ),
        ("MAX_BINDING_BASE64_LEN", MAX_BINDING_BASE64_LEN as u64),
        ("MAX_BINDING_FUTURE_SKEW_S", MAX_BINDING_FUTURE_SKEW_S),
        ("MAX_DOMAIN_BYTES", MAX_DOMAIN_BYTES as u64),
        ("MAX_HANDLE_BYTES", MAX_HANDLE_BYTES as u64),
        ("MAX_USERNAME_BYTES", MAX_USERNAME_BYTES as u64),
        ("MIN_HANDLE_BYTES", MIN_HANDLE_BYTES as u64),
        ("NONCE_LEN", NONCE_LEN as u64),
        ("PASSWORD_LEN", PASSWORD_LEN as u64),
        ("PHRASE_WORDS", PHRASE_WORDS as u64),
        ("PUBLIC_KEY_LEN", PUBLIC_KEY_LEN as u64),
        ("SEED_LEN", SEED_LEN as u64),
        ("SERVER_CHALLENGE_TIMEOUT_S", SERVER_CHALLENGE_TIMEOUT_S),
        ("SIGNATURE_LEN", SIGNATURE_LEN as u64),
        ("SIGN_LOCKOUT_S", SIGN_LOCKOUT_S),
        ("SIGN_MIN_INTERVAL_S", SIGN_MIN_INTERVAL_S),
        (
            "SIGN_WRONG_PASSWORD_LIMIT",
            SIGN_WRONG_PASSWORD_LIMIT.into(),
        ),
        ("WELL_KNOWN_FETCH_TIMEOUT_S", WELL_KNOWN_FETCH_TIMEOUT_S),
        ("WELL_KNOWN_VERSION", WELL_KNOWN_VERSION.into()),
    ];
    for (name, value) in ours {
        assert_eq!(number(c, name), value, "{name}");
    }
    assert_eq!(text(c, "SERVER_KEY_ALG"), SERVER_KEY_ALG);
    let listed = c.as_object().unwrap().len();
    assert_eq!(
        listed,
        ours.len() + 1,
        "a contract constant is not checked here"
    );
}

#[test]
fn every_domain_label_matches_the_contract() {
    let d = &kat()["domains"];
    let ours: [(&str, &[u8]); 10] = [
        ("assertion_domain", ASSERTION_DOMAIN),
        ("binding_domain", BINDING_DOMAIN),
        ("challenge_domain", CHALLENGE_DOMAIN),
        ("fingerprint_domain", FINGERPRINT_DOMAIN),
        ("match_phrase_domain", MATCH_PHRASE_DOMAIN),
        ("password_subkey_label", PASSWORD_SUBKEY_LABEL),
        ("server_key_label", SERVER_KEY_LABEL),
        ("server_password_domain", SERVER_PASSWORD_DOMAIN),
        ("sign_subkey_label", SIGN_SUBKEY_LABEL),
        ("well_known_domain", WELL_KNOWN_DOMAIN),
    ];
    for (name, value) in ours {
        assert_eq!(text(d, name).as_bytes(), value, "{name}");
    }
    // `separator` is prose, not a label.
    assert_eq!(d.as_object().unwrap().len(), ours.len() + 1);
}

#[test]
fn every_canonical_domain_vector() {
    for case in kat()["canonical_domain"].as_array().unwrap() {
        let dialled = text(case, "dialled");
        let got = canonical_domain(dialled);
        match case.get("canonical") {
            Some(expected) => {
                let got = got.unwrap_or_else(|e| panic!("{dialled:?}: {e:?}"));
                assert_eq!(got.as_str(), expected.as_str().unwrap(), "{dialled:?}");
                // Idempotent.
                assert_eq!(canonical_domain(got.as_str()).unwrap(), got);
            }
            None => assert_eq!(
                variant(&got.expect_err(dialled)),
                text(case, "error"),
                "{dialled:?}"
            ),
        }
    }
}

#[test]
fn every_validate_handle_vector() {
    for case in kat()["validate_handle"].as_array().unwrap() {
        let handle = text(case, "handle");
        let got = validate_handle(handle);
        assert_eq!(got.is_ok(), case["ok"].as_bool().unwrap(), "{handle:?}");
        if let Err(e) = got {
            assert_eq!(variant(&e), text(case, "error"), "{handle:?}");
        }
    }
}

/// Vectors that contradict the contract's own prose and schema, with what the crate does
/// instead. `identity-kat.json`'s "exactly 255 bytes" username puts a 182-byte label in its
/// domain; identity-api.md §3 (`[a-z0-9-]{1,63}`) and `well-known-citadel.schema.json`
/// (`{0,62}` after the first character) both cap a label at 63, as DNS does. Reported to C0.
/// Every entry must still match a vector, so a corrected contract turns this test red until the
/// entry is removed.
const CONTRACT_DEFECTS: [(&str, &str); 1] = [("exactly 255 bytes", "Domain")];

#[test]
fn every_parse_username_vector() {
    let mut defects_seen = 0;
    for case in kat()["parse_username"].as_array().unwrap() {
        let username = text(case, "username");
        let got = parse_username(username);
        let note = case.get("note").and_then(Value::as_str).unwrap_or("");
        if let Some((_, refusal)) = CONTRACT_DEFECTS.iter().find(|(n, _)| *n == note) {
            assert_eq!(variant(&got.expect_err(username)), *refusal, "{note}");
            assert!(
                case.get("v2").is_some(),
                "{note}: the defect is no longer in the contract"
            );
            defects_seen += 1;
            continue;
        }
        if let Some(v2) = case.get("v2") {
            assert_eq!(
                got,
                Ok(Username::V2 {
                    handle: text(v2, "handle"),
                    domain: text(v2, "domain"),
                }),
                "{username:?}"
            );
        } else if let Some(legacy) = case.get("legacy") {
            assert_eq!(got, Ok(Username::Legacy(legacy.as_str().unwrap())));
        } else {
            let e = got.expect_err(username);
            assert_eq!(variant(&e), text(case, "error"), "{username:?}");
        }
    }
    assert_eq!(defects_seen, CONTRACT_DEFECTS.len());
}

/// The 255-byte cap itself, with labels the contract allows: 64 + 1 + 190 parses, 256 does not.
#[test]
fn the_username_cap_is_255_bytes_with_valid_labels() {
    let handle = "a".repeat(64);
    let domain = format!(
        "{}.{}.{}.example",
        "d".repeat(63),
        "d".repeat(63),
        "d".repeat(54)
    );
    assert_eq!(domain.len(), MAX_DOMAIN_BYTES);
    let username = format!("{handle}@{domain}");
    assert_eq!(username.len(), MAX_USERNAME_BYTES);
    assert_eq!(
        parse_username(&username),
        Ok(Username::V2 {
            handle: &handle,
            domain: &domain
        })
    );
    let over = format!("{handle}a@{domain}");
    assert_eq!(variant(&parse_username(&over).unwrap_err()), "TooLong");
}

/// An `accepted` entry that is not a canonical domain (`b@c`) cannot be a `CanonicalDomain`, so
/// it is dropped: the type makes such an entry unrepresentable, and the vector's `shown` holds.
#[test]
fn every_display_handle_vector() {
    for case in kat()["display_handle"].as_array().unwrap() {
        let accepted: Vec<CanonicalDomain> = strings(&case["accepted"])
            .into_iter()
            .filter_map(|d| canonical_domain(d).ok().filter(|c| c.as_str() == d))
            .collect();
        let username = text(case, "username");
        assert_eq!(
            display_handle(username, &accepted),
            text(case, "shown"),
            "{username:?}"
        );
    }
}

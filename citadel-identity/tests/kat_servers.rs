//! `identity-kat.json` `servers.*` and `fixtures/well-known-citadel/` (identity-api.md §6,
//! `well-known-citadel.schema.json`): the server keys, the published documents (their signed
//! CBOR and their signatures, byte for byte), every invalid fixture refused for its own reason,
//! and the challenge with the replays that must fail.
//!
//! Control: drop `cid` from `challenge_message` → the cross-cid replay verifies and
//! `the_challenge_vector_and_its_replays` goes red.

mod common;

use citadel_identity::*;
use common::kat::{hex, hex32, load, read_bytes, strings, text, to_hex, variant};
use serde_json::Value;

fn servers() -> Vec<(String, Value)> {
    let kat = load("identity-kat.json");
    let map = kat["servers"].as_object().unwrap();
    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

#[test]
fn every_server_key_vector() {
    for (name, s) in servers() {
        let key = ServerKey::from_seed(&hex32(&s["key"], "seed_hex"));
        assert_eq!(
            key.public_key().to_base64url(),
            text(&s["key"], "pk_b64url"),
            "{name}"
        );
        assert_eq!(
            to_hex(key.public_key().as_bytes()),
            text(&s["key"], "pk_hex")
        );
        assert_eq!(
            fingerprint(key.public_key()),
            text(&s["key"], "fp"),
            "{name}"
        );
    }
}

#[test]
fn every_published_document_parses_and_resigns_to_the_same_bytes() {
    for (name, s) in servers() {
        let wk = &s["well_known"];
        let json = read_bytes(text(wk, "document"));
        let canonical = canonical_domain(&name).unwrap();
        let doc = parse_well_known(&json, &canonical).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(doc.canonical_domain, canonical);

        let message = well_known_message(&doc).unwrap();
        let expected = [WELL_KNOWN_DOMAIN, &[0], &hex(wk, "signed_cbor_hex")].concat();
        assert_eq!(to_hex(&message), to_hex(&expected), "{name}");

        let key = ServerKey::from_seed(&hex32(&s["key"], "seed_hex"));
        let ours: Value = serde_json::from_slice(&key.sign_well_known(&doc).unwrap()).unwrap();
        let published: Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(
            ours, published,
            "{name}: the deterministic signature differs"
        );

        if let Some(accept) = wk.get("accept_fetched_from") {
            for from in strings(accept) {
                assert!(parse_well_known(&json, &canonical_domain(from).unwrap()).is_ok());
            }
        }
        if let Some(refuse) = wk.get("refuse_fetched_from") {
            for from in strings(refuse) {
                let got = parse_well_known(&json, &canonical_domain(from).unwrap());
                assert!(
                    matches!(got, Err(WellKnownError::NotThisServer { .. })),
                    "{from}"
                );
            }
        }
    }
}

/// Each invalid fixture, with the check that must refuse it (so a fixture refused for the
/// wrong reason, e.g. a signature failure masking a missed shape rule, is still red).
const INVALID: [(&str, &str); 11] = [
    ("invalid-former-not-array.json", "Json"),
    ("invalid-issued-at-string.json", "Json"),
    ("invalid-missing-sig.json", "Json"),
    ("invalid-pk-too-short.json", "Signature"),
    ("invalid-sig-padded.json", "Signature"),
    ("invalid-unknown-field.json", "Json"),
    ("invalid-unknown-nested-field.json", "Json"),
    ("invalid-uppercase-domain.json", "Domain"),
    ("invalid-version-2.json", "Version"),
    ("invalid-wrong-alg.json", "Alg"),
    ("invalid-ws-endpoint.json", "Endpoint"),
];

#[test]
fn every_fixture_is_classified_and_every_invalid_one_refused_for_its_reason() {
    let dir = common::kat::contract_path("fixtures/well-known-citadel");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    let invalid: Vec<String> = names
        .iter()
        .filter(|n| n.starts_with("invalid-"))
        .cloned()
        .collect();
    let listed: Vec<String> = INVALID.iter().map(|(f, _)| f.to_string()).collect();
    assert_eq!(
        invalid, listed,
        "a fixture is missing from INVALID, or INVALID names a ghost"
    );
    assert!(names
        .iter()
        .all(|n| n.starts_with("invalid-") || n.starts_with("valid-")));

    let from = canonical_domain("main.work.avarok.net").unwrap();
    for (file, reason) in INVALID {
        let json = read_bytes(&format!("fixtures/well-known-citadel/{file}"));
        let error = parse_well_known(&json, &from).expect_err(file);
        assert_eq!(variant(&error), reason, "{file}: {error}");
    }
}

/// Schema rules no fixture exercises: `issued_at` in `[1700000000, 4102444800]`, at most 16
/// former domains, a lowercase `wss` of at most 256 characters. Each mutation of the published
/// document must be refused as a shape error, before its (now broken) signature is looked at.
#[test]
fn schema_bounds_are_shape_errors() {
    let json = read_bytes("fixtures/well-known-citadel/valid-hosted-tenant.json");
    let from = canonical_domain("main.work.avarok.net").unwrap();
    let mutate = |edit: &dyn Fn(&mut Value)| {
        let mut value: Value = serde_json::from_slice(&json).unwrap();
        edit(&mut value);
        parse_well_known(&serde_json::to_vec(&value).unwrap(), &from)
    };
    type Edit<'a> = &'a dyn Fn(&mut Value);
    let cases: [(&str, Edit); 5] = [
        ("issued_at too early", &|v| {
            v["issued_at"] = 1_699_999_999u64.into()
        }),
        ("issued_at too late", &|v| {
            v["issued_at"] = 4_102_444_801u64.into()
        }),
        ("17 former domains", &|v| {
            v["former_domains"] = (0..17).map(|i| format!("f{i}.example")).collect()
        }),
        ("uppercase wss", &|v| {
            v["citadel"]["wss"] = "wss://MAIN.work.avarok.net/citadel".into()
        }),
        ("long wss", &|v| {
            v["citadel"]["wss"] = format!("wss://main.work.avarok.net/{}", "a".repeat(240)).into()
        }),
    ];
    for (case, edit) in cases {
        let error = mutate(edit).expect_err(case);
        assert_eq!(variant(&error), "Json", "{case}: {error}");
    }
    // The boundaries themselves are shape-valid (refused only by the signature).
    let error = mutate(&|v| v["issued_at"] = 1_700_000_000u64.into()).unwrap_err();
    assert_eq!(variant(&error), "Signature");
}

#[test]
fn the_challenge_vector_and_its_replays() {
    let kat = load("identity-kat.json");
    let s = &kat["servers"]["main.work.avarok.net"];
    let c = &s["challenge"];
    let key = ServerKey::from_seed(&hex32(&s["key"], "seed_hex"));
    let domain = canonical_domain(text(c, "domain")).unwrap();
    let cid: u64 = text(c, "cid").parse().unwrap();
    let nonce = hex32(c, "nonce_hex");
    assert_eq!(
        to_hex(&challenge_message(&domain, cid, &nonce)),
        text(c, "message_hex")
    );
    let sig = key.sign_challenge(&domain, cid, &nonce);
    assert_eq!(to_hex(&sig), text(c, "sig_hex"));
    let pk = key.public_key();
    assert_eq!(
        verify_server_challenge(pk, &domain, cid, &nonce, &sig),
        Ok(())
    );

    let replays = s["challenge_replays_that_must_fail"].as_array().unwrap();
    assert_eq!(replays.len(), 3);
    for replay in replays {
        let cid = replay
            .get("cid")
            .map_or(cid, |v| v.as_str().unwrap().parse().unwrap());
        let domain = replay.get("domain").map_or(domain.clone(), |v| {
            canonical_domain(v.as_str().unwrap()).unwrap()
        });
        let nonce = replay
            .get("nonce_hex")
            .map_or(nonce, |_| hex32(replay, "nonce_hex"));
        let got = verify_server_challenge(pk, &domain, cid, &nonce, &sig);
        assert!(got.is_err(), "replay {replay} verified");
    }
}

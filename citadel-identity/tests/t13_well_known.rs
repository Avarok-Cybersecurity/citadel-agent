//! T13 (rev b): `/.well-known/citadel`. A valid document parses; `sig` covers every field but
//! itself; a fetched host outside canonical ∪ former, an unknown field, a non-wss endpoint, a
//! changed key or an older document is refused.
//!
//! Control: skip the signature check in `parse_well_known` → a document with a forged key and
//! a tampered field is accepted, and `any_edit_breaks_the_self_signature` goes red.

mod common;

use citadel_identity::*;
use common::{domain, former_domain, main_domain, ATTACKER_SEED, NOW, SERVER_SEED};
use serde_json::{json, Value};

fn doc(key: &ServerKey) -> WellKnownCitadel {
    WellKnownCitadel {
        v: 1,
        canonical_domain: main_domain(),
        citadel_wss: Some("wss://main.work.avarok.net/citadel".to_string()),
        server_key: key.public_key().clone(),
        former_domains: vec![former_domain()],
        issued_at_s: NOW,
    }
}

fn served() -> Value {
    let key = ServerKey::from_seed(&SERVER_SEED);
    serde_json::from_slice(&key.sign_well_known(&doc(&key)).unwrap()).unwrap()
}

fn parse(value: &Value, from: &CanonicalDomain) -> Result<WellKnownCitadel, WellKnownError> {
    parse_well_known(&serde_json::to_vec(value).unwrap(), from)
}

#[test]
fn a_signed_document_parses_from_the_canonical_and_a_former_domain() {
    let key = ServerKey::from_seed(&SERVER_SEED);
    let value = served();
    assert_eq!(parse(&value, &main_domain()).unwrap(), doc(&key));
    assert_eq!(parse(&value, &former_domain()).unwrap(), doc(&key));
    assert_eq!(value["server_key"]["alg"], "ML-DSA-65");
    assert_eq!(
        base64url_decode(value["server_key"]["pk"].as_str().unwrap())
            .unwrap()
            .len(),
        PUBLIC_KEY_LEN
    );
}

#[test]
fn a_host_outside_canonical_and_former_is_refused() {
    assert!(matches!(
        parse(&served(), &domain("evil.example")),
        Err(WellKnownError::NotThisServer { .. })
    ));
}

#[test]
fn any_edit_breaks_the_self_signature() {
    let edits: [(&str, Value); 4] = [
        ("canonical_domain", json!("main.work.avarok.org")),
        (
            "former_domains",
            json!(["main-old.work.avarok.net", "evil.example"]),
        ),
        ("issued_at", json!(NOW + 1)),
        ("citadel", json!({ "wss": "wss://evil.example/citadel" })),
    ];
    for (field, value) in edits {
        let mut edited = served();
        edited[field] = value;
        let from = domain("main-old.work.avarok.net");
        assert!(
            matches!(
                parse(&edited, &from),
                Err(WellKnownError::Signature(SignatureError::Invalid))
            ),
            "{field} edited and accepted"
        );
    }
    let mut forged_key = served();
    let attacker = ServerKey::from_seed(&ATTACKER_SEED);
    forged_key["server_key"]["pk"] = json!(attacker.public_key().to_base64url());
    assert!(matches!(
        parse(&forged_key, &main_domain()),
        Err(WellKnownError::Signature(SignatureError::Invalid))
    ));
}

#[test]
fn the_signature_covers_the_deterministic_cbor_of_every_field_but_sig() {
    let key = ServerKey::from_seed(&SERVER_SEED);
    let message = well_known_message(&doc(&key)).unwrap();
    assert!(message.starts_with(b"citadel-well-known-v1\0\xa6"));
    // Shortest-key-first: "v" leads the six-entry map.
    assert_eq!(&message[22..25], &[0xa6, 0x61, b'v']);
}

#[test]
fn unknown_fields_wrong_alg_version_and_endpoint_are_refused() {
    let mut extra = served();
    extra["note"] = json!("unsigned");
    assert!(matches!(
        parse(&extra, &main_domain()),
        Err(WellKnownError::Json(_))
    ));
    let mut nested = served();
    nested["citadel"]["ws"] = json!("ws://main.work.avarok.net");
    assert!(matches!(
        parse(&nested, &main_domain()),
        Err(WellKnownError::Json(_))
    ));
    let mut alg = served();
    alg["server_key"]["alg"] = json!("Ed25519");
    assert!(matches!(
        parse(&alg, &main_domain()),
        Err(WellKnownError::Alg(_))
    ));
    let mut upper = served();
    upper["canonical_domain"] = json!("Main.work.avarok.net");
    assert!(matches!(
        parse(&upper, &main_domain()),
        Err(WellKnownError::Domain(_))
    ));

    let key = ServerKey::from_seed(&SERVER_SEED);
    let mut v2 = doc(&key);
    v2.v = 2;
    assert_eq!(key.sign_well_known(&v2), Err(WellKnownError::Version(2)));
    let mut plain = doc(&key);
    plain.citadel_wss = Some("ws://main.work.avarok.net/citadel".to_string());
    assert!(matches!(
        key.sign_well_known(&plain),
        Err(WellKnownError::Endpoint(_))
    ));
    let mut twice = doc(&key);
    twice.former_domains.push(main_domain());
    assert!(matches!(
        key.sign_well_known(&twice),
        Err(WellKnownError::Domain(_))
    ));
    let other = ServerKey::from_seed(&ATTACKER_SEED);
    assert!(matches!(
        other.sign_well_known(&doc(&key)),
        Err(WellKnownError::KeyChanged { .. })
    ));
}

#[test]
fn a_refresh_must_keep_the_pinned_key_and_not_go_back_in_time() {
    let key = ServerKey::from_seed(&SERVER_SEED);
    let current = doc(&key);
    assert_eq!(check_against_pin(&current, key.public_key(), NOW), Ok(()));
    assert_eq!(
        check_against_pin(&current, key.public_key(), NOW - 1),
        Ok(())
    );
    assert!(matches!(
        check_against_pin(&current, key.public_key(), NOW + 1),
        Err(WellKnownError::Stale { .. })
    ));
    let attacker = ServerKey::from_seed(&ATTACKER_SEED);
    let swapped = WellKnownCitadel {
        server_key: attacker.public_key().clone(),
        ..doc(&key)
    };
    match check_against_pin(&swapped, key.public_key(), NOW) {
        Err(WellKnownError::KeyChanged {
            pinned_fp,
            offered_fp,
        }) => {
            assert_eq!(pinned_fp, fingerprint(key.public_key()));
            assert_eq!(offered_fp, fingerprint(attacker.public_key()));
        }
        other => panic!("a swapped key was {other:?}"),
    }
}

//! `sign-in-kat.json` (sign-in-with-citadel.md §1, §3, §6; identity-api.md §7): the challenge,
//! the match phrase, the device label, the issuer key and the assertion byte for byte, and the
//! `must_fail` cases the crate decides (audience, nonce, expiry). The code, link and allowlist
//! cases are the issuer's (I5), not this crate's.
//!
//! Control: drop the audience comparison in `verify_assertion` → the `mismatched audience`
//! case verifies and `the_must_fail_cases_the_crate_decides` goes red.

mod common;

use citadel_identity::*;
use common::kat::{hex, hex32, load, number, strings, text, to_hex};
use serde_json::Value;
use sha3::{Digest, Sha3_256};

fn kat() -> Value {
    load("sign-in-kat.json")
}

fn identity_a() -> Identity {
    let ids = load("identity-kat.json");
    Identity::from_seed(&hex32(&ids["identities"]["A"], "seed_hex"))
}

fn assertion(a: &Value) -> SignInAssertion {
    SignInAssertion {
        identity: MlDsa65Pk::from_base64url(text(a, "identity_pk_b64url")).unwrap(),
        device: text(a, "device").to_string(),
        aud: text(a, "aud").to_string(),
        exp_s: number(a, "exp_s"),
        nonce: hex32(a, "nonce_hex"),
    }
}

#[test]
fn the_challenge_is_the_verifiers_sha3() {
    let k = kat();
    let challenge = Sha3_256::digest(hex(&k, "verifier_hex"));
    assert_eq!(to_hex(&challenge), text(&k, "challenge_hex"));
    assert_eq!(base64url_encode(&challenge), text(&k, "challenge_b64url"));
}

#[test]
fn the_match_phrase_vector() {
    let k = kat();
    let challenge = hex32(&k, "challenge_hex");
    let m = &k["match_phrase"];
    let h = Sha3_256::digest([MATCH_PHRASE_DOMAIN, &[0], &challenge].concat());
    assert_eq!(to_hex(&h), text(m, "hash_hex"));
    assert_eq!(match_phrase(&challenge).to_vec(), strings(&m["words"]));
    // The indices are the first 33 bits of h, eleven at a time.
    let bits = u64::from_be_bytes(h[..8].try_into().unwrap());
    let indices: Vec<u64> = (0..3).map(|i| (bits >> (53 - 11 * i)) & 0x7ff).collect();
    let expected: Vec<u64> = m["indices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    assert_eq!(indices, expected);
}

#[test]
fn the_device_label_names_the_endpoint_key() {
    let d = &kat()["device"];
    let pk = hex(d, "device_sign_pk_hex");
    assert_eq!(pk.len(), PUBLIC_KEY_LEN);
    let label = format!("macbook-pro#{}", base64url_encode(&Sha3_256::digest(&pk)));
    assert_eq!(label, text(d, "value"));
    assert_eq!(text(&kat()["assertion"], "device"), label);
}

#[test]
fn the_issuer_key_and_the_assertion_byte_for_byte() {
    let k = kat();
    let issuer = ServerKey::from_seed(&hex32(&k["issuer"]["key"], "seed_hex"));
    assert_eq!(
        issuer.public_key().to_base64url(),
        text(&k["issuer"]["key"], "pk_b64url")
    );
    assert_eq!(
        fingerprint(issuer.public_key()),
        text(&k["issuer"]["key"], "fp")
    );

    let a = &k["assertion"];
    let claim = assertion(a);
    assert_eq!(
        claim.identity,
        *identity_a().public_key(),
        "the identity is A's full key"
    );
    assert_eq!(fingerprint(&claim.identity), text(a, "identity_id_fp"));
    assert_eq!(claim.nonce, hex32(&k, "challenge_hex"), "nonce = challenge");

    let message = assertion_message(&claim).unwrap();
    assert_eq!(to_hex(&message), text(a, "message_hex"));
    let prefix = [ASSERTION_DOMAIN, &[0]].concat();
    assert_eq!(to_hex(&message[prefix.len()..]), text(a, "body_hex"));

    let blob = issuer.sign_assertion(&claim).unwrap();
    assert_eq!(blob.len() as u64, number(a, "blob_len"));
    assert_eq!(base64url_encode(&blob), text(a, "blob_b64url"));
    assert_eq!(
        to_hex(&blob[blob.len() - SIGNATURE_LEN..]),
        text(a, "sig_hex")
    );

    let stored = base64url_decode(text(a, "blob_b64url")).unwrap();
    let now = claim.exp_s - 1;
    let got = verify_assertion(&stored, issuer.public_key(), &claim.aud, &claim.nonce, now);
    assert_eq!(got, Ok(claim));
}

#[test]
fn the_must_fail_cases_the_crate_decides() {
    let k = kat();
    let a = &k["assertion"];
    let claim = assertion(a);
    let issuer = MlDsa65Pk::from_base64url(text(&k["issuer"]["key"], "pk_b64url")).unwrap();
    let blob = base64url_decode(text(a, "blob_b64url")).unwrap();
    let now = claim.exp_s - 1;
    let mut decided = 0;
    for case in k["must_fail"].as_array().unwrap() {
        let got = if let Some(aud) = case.get("verify_with_aud") {
            verify_assertion(&blob, &issuer, aud.as_str().unwrap(), &claim.nonce, now)
        } else if case.get("verifier_hex").is_some() {
            let nonce: [u8; 32] = Sha3_256::digest(hex(case, "verifier_hex")).into();
            verify_assertion(&blob, &issuer, &claim.aud, &nonce, now)
        } else if let Some(at) = case.get("now_s") {
            verify_assertion(
                &blob,
                &issuer,
                &claim.aud,
                &claim.nonce,
                at.as_u64().unwrap(),
            )
        } else {
            continue; // the issuer's code store or allowlist decides it
        };
        decided += 1;
        assert!(got.is_err(), "{}: verified", text(case, "case"));
    }
    assert_eq!(decided, 3);
    // The issuer's key is not the identity's: an assertion checked against A is refused.
    let wrong = identity_a().public_key().clone();
    let got = verify_assertion(&blob, &wrong, &claim.aud, &claim.nonce, now);
    assert!(matches!(got, Err(AssertionError::Signature(_))));
}

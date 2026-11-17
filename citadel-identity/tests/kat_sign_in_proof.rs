//! `sign-in-kat.json` `proof_constants`, `proof` and `proof_must_fail`
//! (identity-api.md §7a; sign-in-with-citadel.md §1a): identity A's proof byte for byte and every
//! case the crate decides refused for its own reason. The issuer-only document: `kat_issuer_well_known.rs`.
//!
//! Controls: drop the `origin` comparison in `verify_proof` → identity A's valid signature over
//! the phishing origin verifies and `a_valid_signature_over_a_phishing_origin_is_refused` goes red;
//! drop `origin` from the signed body → the same test (and the byte-for-byte vector) goes red.

mod common;

use citadel_identity::*;
use common::kat::{hex, hex32, load, number, text, to_hex, variant};
use serde_json::Value;

fn kat() -> Value {
    load("sign-in-kat.json")
}

fn identity(name: &str) -> Value {
    load("identity-kat.json")["identities"][name].clone()
}

fn identity_a() -> Identity {
    Identity::from_seed(&hex32(&identity("A"), "seed_hex"))
}

/// The issuer's own expectations: the vector's issuer, origin, aud, nonce and challenge.
fn verify_as_issuer(blob: &[u8]) -> Result<SignInProof, ProofError> {
    let p = &kat()["proof"];
    verify_proof(
        blob,
        text(p, "issuer"),
        text(p, "origin"),
        text(p, "aud"),
        &hex32(p, "nonce_hex"),
        &hex32(p, "challenge_hex"),
    )
}

fn vector_blob() -> Vec<u8> {
    base64url_decode(text(&kat()["proof"], "blob_b64url")).unwrap()
}

#[test]
fn the_proof_constants() {
    let c = &kat()["proof_constants"];
    assert_eq!(PROOF_DOMAIN, text(c, "PROOF_DOMAIN").as_bytes());
    assert_eq!(u64::from(PROOF_VERSION), number(c, "PROOF_VERSION"));
    assert_eq!(PROOF_FIXED_LEN as u64, number(c, "PROOF_FIXED_LEN"));
}

#[test]
fn identity_as_proof_byte_for_byte_and_it_verifies() {
    let k = kat();
    let p = &k["proof"];
    assert_eq!(text(p, "challenge_hex"), text(&k, "challenge_hex"));
    assert_eq!(text(p, "nonce_hex"), text(&k["attempt"], "nonce_hex"));
    assert_eq!(text(p, "issuer"), text(&k["issuer"], "domain"));
    assert_eq!(text(p, "origin"), text(&k["issuer"], "origin"));

    let a = identity_a();
    assert_eq!(a.public_key().to_base64url(), text(p, "identity_pk_b64url"));
    assert_eq!(a.id_fp(), text(p, "identity_id_fp"));
    let blob = a
        .sign_proof(
            text(p, "issuer"),
            text(p, "aud"),
            text(p, "origin"),
            &hex32(p, "nonce_hex"),
            &hex32(p, "challenge_hex"),
        )
        .unwrap();
    let strings = text(p, "issuer").len() + text(p, "aud").len() + text(p, "origin").len();
    assert_eq!(blob.len(), PROOF_FIXED_LEN + strings);
    assert_eq!(blob.len() as u64, number(p, "blob_len"));
    assert_eq!(base64url_encode(&blob), text(p, "blob_b64url"));
    assert_eq!(
        to_hex(&blob[1..blob.len() - SIGNATURE_LEN]),
        text(p, "body_hex")
    );
    assert_eq!(
        to_hex(&blob[blob.len() - SIGNATURE_LEN..]),
        text(p, "sig_hex")
    );

    let proof = verify_as_issuer(&vector_blob()).unwrap();
    assert_eq!(
        proof.identity,
        *a.public_key(),
        "the subject is A's full key"
    );
    assert_eq!(
        to_hex(&proof_message(&proof).unwrap()),
        text(p, "message_hex")
    );

    // P1's control: one flipped body byte is a signature failure, not a field mismatch.
    let mut flipped = vector_blob();
    flipped[40] ^= 1;
    assert!(matches!(
        verify_as_issuer(&flipped),
        Err(ProofError::Signature(_))
    ));
}

#[test]
fn a_valid_signature_over_a_phishing_origin_is_refused() {
    let case = &kat()["proof_must_fail"][0];
    assert_eq!(text(case, "reason"), "wrong_origin");
    let blob = base64url_decode(text(case, "blob_b64url")).unwrap();
    assert_eq!(blob.len() as u64, number(case, "blob_len"));
    // The signature is A's and it is valid: on the phishing origin's own terms it verifies.
    let p = &kat()["proof"];
    let honest = verify_proof(
        &blob,
        text(p, "issuer"),
        text(case, "origin"),
        text(p, "aud"),
        &hex32(p, "nonce_hex"),
        &hex32(p, "challenge_hex"),
    );
    assert!(honest.is_ok(), "{honest:?}");
    // The issuer accepts only its own origin.
    assert_eq!(
        verify_as_issuer(&blob),
        Err(ProofError::WrongOrigin {
            expected: text(p, "origin").to_string(),
            found: text(case, "origin").to_string(),
        })
    );
}

/// Reason in the KAT → the crate's variant. The `attempt_*` reasons are the issuer's store's.
fn expected_variant(reason: &str) -> Option<&'static str> {
    Some(match reason {
        "wrong_origin" => "WrongOrigin",
        "bad_signature" => "Signature",
        "wrong_issuer" => "WrongIssuer",
        "audience_mismatch" => "WrongAudience",
        "nonce_mismatch" => "WrongNonce",
        "challenge_mismatch" => "WrongChallenge",
        "attempt_proven" | "attempt_expired" => return None,
        other => panic!("unclassified proof_must_fail reason {other:?}"),
    })
}

#[test]
fn every_proof_must_fail_case_the_crate_decides_for_its_own_reason() {
    let k = kat();
    let p = &k["proof"];
    let body = hex(p, "body_hex");
    let sig = hex(p, "sig_hex");
    let blob_of = |body: &[u8], sig: &[u8]| [&[PROOF_VERSION][..], body, sig].concat();
    let field = |case: &Value, key: &str, fallback: &str| {
        case.get(key)
            .map_or(fallback.to_string(), |v| v.as_str().unwrap().to_string())
    };
    let mut decided = 0;
    for case in k["proof_must_fail"].as_array().unwrap() {
        let name = text(case, "case");
        let Some(reason) = expected_variant(text(case, "reason")) else {
            continue;
        };
        let blob = if case.get("blob_b64url").is_some() {
            base64url_decode(text(case, "blob_b64url")).unwrap()
        } else if case.get("sig_hex").is_some() {
            blob_of(&body, &hex(case, "sig_hex"))
        } else if name.starts_with("another identity") {
            let b = MlDsa65Pk::from_base64url(text(&identity("B"), "pk_b64url")).unwrap();
            let mut swapped = body.clone();
            let at = swapped.len() - PUBLIC_KEY_LEN;
            swapped[at..].copy_from_slice(b.as_bytes());
            blob_of(&swapped, &sig)
        } else {
            blob_of(&body, &sig)
        };
        let nonce = case
            .get("verify_with_nonce_hex")
            .map_or(hex32(p, "nonce_hex"), |_| {
                hex32(case, "verify_with_nonce_hex")
            });
        let challenge = case
            .get("verify_with_challenge_hex")
            .map_or(hex32(p, "challenge_hex"), |_| {
                hex32(case, "verify_with_challenge_hex")
            });
        let got = verify_proof(
            &blob,
            &field(case, "verify_with_issuer", text(p, "issuer")),
            text(p, "origin"),
            &field(case, "verify_with_aud", text(p, "aud")),
            &nonce,
            &challenge,
        );
        let error = got.expect_err(name);
        assert_eq!(variant(&error), reason, "{name}: {error}");
        decided += 1;
    }
    assert_eq!(decided, 7, "every crate-decided case ran");
}

#[test]
fn proof_fields_out_of_bounds_are_refused_before_signing() {
    let a = identity_a();
    let n = [0u8; 32];
    let long = "a".repeat(MAX_PROOF_FIELD_BYTES + 1);
    for (issuer, aud, origin, name) in [
        ("", "sw-mcp", "https://work.avarok.net", "issuer"),
        (
            "work.avarok.net",
            long.as_str(),
            "https://work.avarok.net",
            "aud",
        ),
        ("work.avarok.net", "sw-mcp", "", "origin"),
    ] {
        assert_eq!(
            a.sign_proof(issuer, aud, origin, &n, &n),
            Err(ProofError::Field(name))
        );
    }
    let mut blob = vector_blob();
    blob[0] = 0x02;
    assert_eq!(verify_as_issuer(&blob), Err(ProofError::Version(2)));
    assert!(matches!(
        verify_as_issuer(&vector_blob()[..100]),
        Err(ProofError::Malformed(_))
    ));
}

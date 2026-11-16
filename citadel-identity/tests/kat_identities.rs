//! `identity-kat.json` `identities.*` (identity-api.md §4, §5, §8): keys, fingerprints, the
//! per-server passwords, both bindings and the recovery phrase, from the published seeds.
//! Signing is deterministic, so the signatures are compared byte for byte, not only verified.
//!
//! Control: swap the HMAC sub-key labels in `Identity::from_seed` → every pk and password
//! differs and `every_identity_vector` goes red.

mod common;

use citadel_identity::*;
use common::kat::{hex, hex32, load, number, strings, text, to_hex};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha3::{Digest, Sha3_256};

fn hmac(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut mac = <Hmac<Sha3_256> as Mac>::new_from_slice(key).unwrap();
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

fn identities() -> Vec<(String, Value)> {
    let kat = load("identity-kat.json");
    let map = kat["identities"].as_object().unwrap();
    assert!(map.len() >= 2, "the contract publishes identities A and B");
    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

#[test]
fn every_identity_vector() {
    for (name, v) in identities() {
        let seed = hex32(&v, "seed_hex");
        // The sub-keys, recomputed independently: the crate keeps neither, so its pk and
        // passwords below are what prove it uses them.
        assert_eq!(
            hmac(&seed, SIGN_SUBKEY_LABEL),
            hex(&v, "xi_hex"),
            "{name} xi"
        );
        assert_eq!(
            hmac(&seed, PASSWORD_SUBKEY_LABEL),
            hex(&v, "k_pw_hex"),
            "{name} k_pw"
        );

        let identity = Identity::from_seed(&seed);
        assert_eq!(
            identity.public_key().to_base64url(),
            text(&v, "pk_b64url"),
            "{name}"
        );
        assert_eq!(
            to_hex(&Sha3_256::digest(identity.pk())),
            text(&v, "pk_sha3_256_hex"),
            "{name}"
        );
        assert_eq!(identity.id_fp(), text(&v, "id_fp"), "{name}");
        assert_eq!(fingerprint(identity.public_key()), text(&v, "id_fp"));
        assert_eq!(
            fingerprint_grouped(identity.public_key()),
            text(&v, "id_fp_grouped")
        );

        for case in v["passwords"].as_array().unwrap() {
            let domain = canonical_domain(text(case, "domain")).unwrap();
            let password = identity.derive_password(&domain, text(case, "username"));
            assert_eq!(password.as_str(), text(case, "password"), "{name} {domain}");
            assert_eq!(password.as_str().len(), PASSWORD_LEN);
            assert_eq!(
                base64url_decode(password.as_str()).unwrap(),
                hex(case, "pw_bytes_hex")
            );
        }
    }
}

fn check_binding(name: &str, identity: &Identity, b: &Value) {
    let username = text(b, "username");
    let domain = canonical_domain(text(b, "domain")).unwrap();
    let issued_at = number(b, "issued_at_s");
    let message = binding_message(username, domain.as_str(), issued_at).unwrap();
    assert_eq!(
        to_hex(&message),
        text(b, "message_hex"),
        "{name} {username}"
    );

    let blob = identity.sign_binding(username, &domain, issued_at).unwrap();
    assert_eq!(blob.as_bytes().len() as u64, number(b, "blob_len"));
    assert_eq!(
        blob.as_bytes().len(),
        BINDING_FIXED_LEN + domain.as_str().len()
    );
    assert_eq!(
        &blob.as_bytes()[blob.as_bytes().len() - SIGNATURE_LEN..],
        hex(b, "sig_hex")
    );
    assert_eq!(
        blob.to_base64url(),
        text(b, "blob_b64url"),
        "{name} {username}"
    );

    let stored = binding_from_base64url(text(b, "blob_b64url")).unwrap();
    let verified =
        verify_binding(&stored, username, std::slice::from_ref(&domain), issued_at).unwrap();
    assert_eq!(verified.id_fp, identity.id_fp());
    assert_eq!(verified.domain, domain);
    assert_eq!(verified.issued_at_s, issued_at);
}

#[test]
fn every_binding_vector_signs_and_verifies_byte_for_byte() {
    for (name, v) in identities() {
        let identity = Identity::from_seed(&hex32(&v, "seed_hex"));
        check_binding(&name, &identity, &v["binding"]);
        check_binding(&name, &identity, &v["legacy_binding"]);
    }
}

#[test]
fn a_binding_is_not_another_identitys_or_another_names() {
    let all = identities();
    let (a, b) = (&all[0].1, &all[1].1);
    let blob = binding_from_base64url(text(&a["binding"], "blob_b64url")).unwrap();
    let domain = canonical_domain(text(&a["binding"], "domain")).unwrap();
    let now = number(&a["binding"], "issued_at_s");
    let verified = verify_binding(
        &blob,
        "alice@acme.example",
        std::slice::from_ref(&domain),
        now,
    )
    .unwrap();
    assert_ne!(verified.id_fp, text(b, "id_fp"));
    // The same blob for the legacy name, or bound under the legacy name's message, is refused.
    assert!(matches!(
        verify_binding(&blob, "john.doe", &[domain], now),
        Err(BindingError::Signature(_))
    ));
}

#[test]
fn every_phrase_vector() {
    for (name, v) in identities() {
        let seed = hex32(&v, "seed_hex");
        let p = &v["phrase"];
        let words = strings(&p["words"]);
        assert_eq!(phrase_encode(&seed).to_vec(), words, "{name}");
        assert_eq!(phrase_decode(&words), Ok(seed), "{name}");
        let check = hex(p, "check_byte_hex")[0];
        assert_eq!(Sha3_256::digest(seed)[0], check);
        // The last 11-bit index is the seed's last 3 bits then the check byte.
        let last = p["indices"].as_array().unwrap()[PHRASE_WORDS - 1]
            .as_u64()
            .unwrap();
        assert_eq!(last & 0xff, u64::from(check));
        assert_eq!(last >> 8, u64::from(seed[31] & 0b111));
    }
}

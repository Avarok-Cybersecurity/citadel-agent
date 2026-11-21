//! T2 (rev b): the identity key, fingerprint and per-server password, recomputed here from the
//! spec's formulas (identity doc §3.2, §3.3) with the primitives directly, so the crate is held
//! to the document and not to itself.
//!
//! Control: reuse `seed` as `xi` (skip the HMAC in `Identity::from_seed`) →
//! `the_signing_key_is_a_sub_key_not_the_seed` goes red.

mod common;

use citadel_identity::*;
use common::{domain, main_domain, OTHER_SEED, SEED};
use hmac::{Hmac, Mac};
use ml_dsa::signature::Keypair;
use ml_dsa::{MlDsa65, Seed, SigningKey};
use sha3::{Digest, Sha3_256};

fn hmac(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut mac = <Hmac<Sha3_256> as Mac>::new_from_slice(key).unwrap();
    mac.update(msg);
    mac.finalize().into_bytes().into()
}

fn mldsa_pk(xi: &[u8; 32]) -> Vec<u8> {
    let mut seed = Seed::default();
    seed.copy_from_slice(xi);
    SigningKey::<MlDsa65>::from_seed(&seed)
        .verifying_key()
        .encode()
        .to_vec()
}

#[test]
fn the_signing_key_is_a_sub_key_not_the_seed() {
    let identity = Identity::from_seed(&SEED);
    let xi = hmac(&SEED, b"citadel-identity-sign-v1");
    assert_eq!(identity.pk().to_vec(), mldsa_pk(&xi));
    assert_ne!(identity.pk().to_vec(), mldsa_pk(&SEED));
}

#[test]
fn the_fingerprint_is_80_bits_of_the_domain_separated_hash() {
    let identity = Identity::from_seed(&SEED);
    let mut hasher = Sha3_256::new();
    hasher.update(b"citadel-identity-fp-v1\0");
    hasher.update(identity.pk());
    let digest: [u8; 32] = hasher.finalize().into();
    assert_eq!(identity.id_fp(), base32_lower_nopad(&digest[..10]));
    assert_eq!(identity.id_fp().len(), 16);
    let grouped = fingerprint_grouped(identity.public_key());
    assert_eq!(grouped.replace('-', ""), identity.id_fp());
    assert_eq!(grouped.len(), 19);
    assert_ne!(Identity::from_seed(&OTHER_SEED).id_fp(), identity.id_fp());
}

#[test]
fn base32_is_rfc_4648_lowercase_unpadded() {
    assert_eq!(base32_lower_nopad(b""), "");
    assert_eq!(base32_lower_nopad(b"f"), "my");
    assert_eq!(base32_lower_nopad(b"fooba"), "mzxw6ytb");
    assert_eq!(base32_lower_nopad(b"foobar"), "mzxw6ytboi");
}

#[test]
fn the_password_follows_section_3_3() {
    let identity = Identity::from_seed(&SEED);
    let d = main_domain();
    let u = make_username("alice", &d).unwrap();
    let k_pw = hmac(&SEED, b"citadel-identity-pw-v1");
    let msg = [
        b"citadel-server-pw-v2\0".as_slice(),
        d.as_str().as_bytes(),
        b"\0",
        u.as_bytes(),
    ]
    .concat();
    let expected = base64url_encode(&hmac(&k_pw, &msg));
    assert_eq!(identity.derive_password(&d, &u).as_str(), expected);
    assert_eq!(
        format!("{:?}", identity.derive_password(&d, &u)),
        "Password(<redacted>)"
    );
}

#[test]
fn derivations_are_deterministic_and_separate_servers() {
    let (a, b) = (Identity::from_seed(&SEED), Identity::from_seed(&SEED));
    assert_eq!(a.pk(), b.pk());
    assert_eq!(a.id_fp(), b.id_fp());
    let main = main_domain();
    let acme = domain("acme.example");
    let on_main = make_username("alice", &main).unwrap();
    let on_acme = make_username("alice", &acme).unwrap();
    let pw_main = a.derive_password(&main, &on_main);
    assert_eq!(
        pw_main.as_str(),
        b.derive_password(&main, &on_main).as_str()
    );
    let pw_acme = a.derive_password(&acme, &on_acme);
    assert_ne!(pw_main.as_str(), pw_acme.as_str());
    // The domain and the username are both inputs, with a separator between them.
    assert_ne!(
        pw_main.as_str(),
        a.derive_password(&main, &on_acme).as_str()
    );
    assert_ne!(
        pw_main.as_str(),
        Identity::from_seed(&OTHER_SEED)
            .derive_password(&main, &on_main)
            .as_str()
    );
    for pw in [&pw_main, &pw_acme] {
        assert_eq!(pw.as_str().len(), PASSWORD_LEN);
        assert!(pw
            .as_str()
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'));
    }
}

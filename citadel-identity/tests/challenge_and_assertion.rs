//! The server challenge (identity doc §3.1, T15's crate half) and the "Sign in with Citadel"
//! assertion (master plan §9a D-B).
//!
//! Controls:
//! - drop `u64be(cid)` from `challenge_message` → an answer for one session verifies for
//!   another, and `a_challenge_answer_binds_nonce_cid_and_domain` goes red;
//! - drop the audience comparison in `verify_assertion` → an assertion for another app is
//!   accepted, and `an_assertion_for_another_audience_is_refused` goes red.

mod common;

use citadel_identity::*;
use common::{domain, main_domain, ATTACKER_SEED, NONCE, NOW, SEED, SERVER_SEED};

const CID: u64 = 0x0102_0304_0506_0708;

#[test]
fn a_challenge_answer_binds_nonce_cid_and_domain() {
    let key = ServerKey::from_seed(&SERVER_SEED);
    let d = main_domain();
    let sig = key.sign_challenge(&d, CID, &NONCE);
    assert_eq!(sig.len(), SIGNATURE_LEN);
    assert_eq!(
        verify_server_challenge(key.public_key(), &d, CID, &NONCE, &sig),
        Ok(())
    );
    let refused = |pk: &MlDsa65Pk, d: &CanonicalDomain, cid: u64, nonce: &[u8; 32]| {
        verify_server_challenge(pk, d, cid, nonce, &sig).is_err()
    };
    assert!(
        refused(key.public_key(), &d, CID + 1, &NONCE),
        "replayed for another cid"
    );
    assert!(
        refused(key.public_key(), &domain("acme.example"), CID, &NONCE),
        "replayed for another domain"
    );
    assert!(
        refused(key.public_key(), &d, CID, &[0; 32]),
        "replayed for another nonce"
    );
    let attacker = ServerKey::from_seed(&ATTACKER_SEED);
    assert!(
        refused(attacker.public_key(), &d, CID, &NONCE),
        "verified by another key"
    );
    assert!(matches!(
        verify_server_challenge(key.public_key(), &d, CID, &NONCE, &sig[1..]),
        Err(ChallengeError::Signature(
            SignatureError::MalformedSignature(_)
        ))
    ));
}

#[test]
fn the_challenge_message_is_the_spec_layout() {
    let d = domain("acme.example");
    let mut expected = b"citadel-server-challenge-v1\0".to_vec();
    expected.extend_from_slice(&NONCE);
    expected.extend_from_slice(&CID.to_be_bytes());
    expected.extend_from_slice(b"acme.example");
    assert_eq!(challenge_message(&d, CID, &NONCE), expected);
}

#[test]
fn the_server_key_is_not_the_identity_key_from_the_same_seed() {
    assert_ne!(
        ServerKey::from_seed(&SEED).public_key().as_bytes(),
        Identity::from_seed(&SEED).pk()
    );
}

fn assertion() -> SignInAssertion {
    SignInAssertion {
        identity: Identity::from_seed(&SEED).public_key().clone(),
        device: "macbook-7f3a".to_string(),
        aud: "sw-mcp".to_string(),
        exp_s: NOW + 600,
        nonce: NONCE,
    }
}

fn signed() -> (ServerKey, Vec<u8>) {
    let issuer = ServerKey::from_seed(&SERVER_SEED);
    let blob = issuer.sign_assertion(&assertion()).unwrap();
    (issuer, blob)
}

#[test]
fn an_assertion_round_trips_for_its_audience_and_nonce() {
    let (issuer, blob) = signed();
    assert_eq!(blob[0], ASSERTION_VERSION);
    assert_eq!(
        verify_assertion(&blob, issuer.public_key(), "sw-mcp", &NONCE, NOW),
        Ok(assertion())
    );
}

#[test]
fn an_assertion_for_another_audience_is_refused() {
    let (issuer, blob) = signed();
    assert!(matches!(
        verify_assertion(&blob, issuer.public_key(), "sw-site", &NONCE, NOW),
        Err(AssertionError::WrongAudience { .. })
    ));
    // Rewriting the audience in place (same length) breaks the signature instead.
    let mut rewritten = blob.clone();
    rewritten[3..9].copy_from_slice(b"sw-ste");
    assert!(matches!(
        verify_assertion(&rewritten, issuer.public_key(), "sw-ste", &NONCE, NOW),
        Err(AssertionError::Signature(SignatureError::Invalid))
    ));
}

#[test]
fn another_nonce_an_expiry_or_another_issuer_is_refused() {
    let (issuer, blob) = signed();
    assert_eq!(
        verify_assertion(&blob, issuer.public_key(), "sw-mcp", &[0; 32], NOW),
        Err(AssertionError::WrongNonce)
    );
    let exp = NOW + 600;
    assert!(verify_assertion(&blob, issuer.public_key(), "sw-mcp", &NONCE, exp - 1).is_ok());
    assert_eq!(
        verify_assertion(&blob, issuer.public_key(), "sw-mcp", &NONCE, exp),
        Err(AssertionError::Expired {
            exp_s: exp,
            now_s: exp
        })
    );
    let attacker = ServerKey::from_seed(&ATTACKER_SEED);
    assert!(matches!(
        verify_assertion(&blob, attacker.public_key(), "sw-mcp", &NONCE, NOW),
        Err(AssertionError::Signature(SignatureError::Invalid))
    ));
    let forged = attacker.sign_assertion(&assertion()).unwrap();
    assert!(verify_assertion(&forged, issuer.public_key(), "sw-mcp", &NONCE, NOW).is_err());
}

#[test]
fn a_malformed_assertion_is_refused() {
    let (issuer, blob) = signed();
    let check = |b: &[u8]| verify_assertion(b, issuer.public_key(), "sw-mcp", &NONCE, NOW);
    assert!(matches!(
        check(&blob[..blob.len() - 1]),
        Err(AssertionError::Malformed(_))
    ));
    assert!(matches!(
        check(&[blob.clone(), vec![0]].concat()),
        Err(AssertionError::Malformed(_))
    ));
    let mut v2 = blob.clone();
    v2[0] = 2;
    assert_eq!(check(&v2), Err(AssertionError::Version(2)));
    let mut identity_bit = blob.clone();
    let at = blob.len() - SIGNATURE_LEN - 1;
    identity_bit[at] ^= 1;
    assert!(matches!(
        check(&identity_bit),
        Err(AssertionError::Signature(SignatureError::Invalid))
    ));
    let empty_aud = SignInAssertion {
        aud: String::new(),
        ..assertion()
    };
    assert_eq!(
        issuer.sign_assertion(&empty_aud),
        Err(AssertionError::Field("aud"))
    );
    let long_device = SignInAssertion {
        device: "d".repeat(256),
        ..assertion()
    };
    assert_eq!(
        issuer.sign_assertion(&long_device),
        Err(AssertionError::Field("device"))
    );
}

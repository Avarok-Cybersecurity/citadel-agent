//! T3 (rev b): the binding blob. Round trip, exact layout and length, every field tampered.
//!
//! Control (the plan's named one): drop the domain-separation prefix (make
//! `codec::domain_separated` return the body alone) → a signature the same key made over the
//! same fields outside this protocol verifies as a binding, and
//! `a_signature_over_the_fields_without_the_binding_domain_is_refused` goes red.

mod common;

use citadel_identity::*;
use common::{domain, former_domain, main_domain, NOW, OTHER_SEED, SEED};
use ml_dsa::signature::Signer;
use ml_dsa::{MlDsa65, Seed, Signature, SigningKey};

const ISSUED: u64 = NOW - 60;

fn alice() -> String {
    make_username("alice", &main_domain()).unwrap()
}

fn blob() -> Vec<u8> {
    let identity = Identity::from_seed(&SEED);
    identity
        .sign_binding(&alice(), &main_domain(), ISSUED)
        .unwrap()
        .as_bytes()
        .to_vec()
}

fn accepted() -> Vec<CanonicalDomain> {
    vec![main_domain(), former_domain()]
}

/// The identity's own ML-DSA key, built from the spec's `xi` outside the crate, signing `msg`.
fn raw_sign(msg: &[u8]) -> Vec<u8> {
    let mut xi = Seed::default();
    xi.copy_from_slice(&hmac_xi());
    let sig: Signature<MlDsa65> = SigningKey::<MlDsa65>::from_seed(&xi).sign(msg);
    sig.encode().to_vec()
}

fn hmac_xi() -> [u8; 32] {
    use hmac::{Hmac, Mac};
    let mut mac = <Hmac<sha3::Sha3_256> as Mac>::new_from_slice(&SEED).unwrap();
    mac.update(b"citadel-identity-sign-v1");
    mac.finalize().into_bytes().into()
}

/// The binding's fields as §3.4 lays them out, built here and not by the crate, without any
/// domain: what another protocol signing the same fields with the same key would sign.
fn fields() -> Vec<u8> {
    let (u, d) = (alice(), main_domain());
    let mut body = (u.len() as u16).to_be_bytes().to_vec();
    body.extend_from_slice(u.as_bytes());
    body.extend_from_slice(&(d.as_str().len() as u16).to_be_bytes());
    body.extend_from_slice(d.as_str().as_bytes());
    body.extend_from_slice(&ISSUED.to_be_bytes());
    body
}

fn with_signature(mut blob: Vec<u8>, sig: &[u8]) -> Vec<u8> {
    let at = blob.len() - SIGNATURE_LEN;
    blob[at..].copy_from_slice(sig);
    blob
}

#[test]
fn a_binding_round_trips_with_the_rev_b_layout() {
    let blob = blob();
    let d = main_domain();
    assert_eq!(blob.len(), 5272 + d.as_str().len());
    assert_eq!(blob[0], 0x02);
    assert_eq!(blob[1..9], ISSUED.to_be_bytes());
    assert_eq!(blob[9..11], (d.as_str().len() as u16).to_be_bytes());
    let identity = Identity::from_seed(&SEED);
    let at = 11 + d.as_str().len();
    assert_eq!(&blob[at..at + PUBLIC_KEY_LEN], identity.pk());
    let verified = verify_binding(&blob, &alice(), &accepted(), NOW).unwrap();
    assert_eq!(verified.id_fp, identity.id_fp());
    assert_eq!(verified.domain, d);
    assert_eq!(verified.issued_at_s, ISSUED);
    let text = BindingBlob::to_base64url(&identity.sign_binding(&alice(), &d, ISSUED).unwrap());
    assert!(text.len() <= MAX_BINDING_BASE64_LEN);
    assert_eq!(binding_from_base64url(&text).unwrap(), blob);
}

#[test]
fn the_message_is_the_domain_and_length_prefixed_fields() {
    let msg = binding_message("alice@acme.example", "acme.example", 1).unwrap();
    let mut expected =
        b"citadel-identity-binding-v2\0\x00\x12alice@acme.example\x00\x0cacme.example".to_vec();
    expected.extend_from_slice(&1u64.to_be_bytes());
    assert_eq!(msg, expected);
}

#[test]
fn a_tampered_or_misdirected_binding_is_refused() {
    let blob = blob();
    let bob = make_username("bob", &main_domain()).unwrap();
    assert!(matches!(
        verify_binding(&blob, &bob, &accepted(), NOW),
        Err(BindingError::Signature(_))
    ));
    assert!(matches!(
        verify_binding(&blob, &alice(), &[domain("acme.example")], NOW),
        Err(BindingError::DomainNotAccepted(_))
    ));
    let mut later = blob.clone();
    later[8] ^= 1;
    assert!(matches!(
        verify_binding(&later, &alice(), &accepted(), NOW),
        Err(BindingError::Signature(_))
    ));
    let mut flipped = blob.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 1;
    assert!(verify_binding(&flipped, &alice(), &accepted(), NOW).is_err());
    let mut other_key = blob.clone();
    other_key[11 + 20..11 + 20 + PUBLIC_KEY_LEN]
        .copy_from_slice(Identity::from_seed(&OTHER_SEED).pk());
    assert!(matches!(
        verify_binding(&other_key, &alice(), &accepted(), NOW),
        Err(BindingError::Signature(_))
    ));
    // The domain swapped for an accepted one of the same length still fails the signature.
    let mut moved = blob.clone();
    moved[11..31].copy_from_slice(b"main.work.avarok.org");
    let both = [main_domain(), domain("main.work.avarok.org")];
    assert!(matches!(
        verify_binding(&moved, &alice(), &both, NOW),
        Err(BindingError::Signature(_))
    ));
}

#[test]
fn version_length_future_and_bad_names_are_refused() {
    let blob = blob();
    let mut v1 = blob.clone();
    v1[0] = 0x01;
    assert_eq!(
        verify_binding(&v1, &alice(), &accepted(), NOW),
        Err(BindingError::Version(1))
    );
    assert!(matches!(
        verify_binding(&blob[..blob.len() - 1], &alice(), &accepted(), NOW),
        Err(BindingError::Malformed(_))
    ));
    let mut longer = blob.clone();
    longer.push(0);
    assert!(matches!(
        verify_binding(&longer, &alice(), &accepted(), NOW),
        Err(BindingError::Malformed(_))
    ));
    let early = ISSUED - MAX_BINDING_FUTURE_SKEW_S - 1;
    assert!(matches!(
        verify_binding(&blob, &alice(), &accepted(), early),
        Err(BindingError::FromTheFuture { .. })
    ));
    assert!(verify_binding(
        &blob,
        &alice(),
        &accepted(),
        ISSUED - MAX_BINDING_FUTURE_SKEW_S
    )
    .is_ok());
    assert!(matches!(
        verify_binding(&blob, "a@b@c", &accepted(), NOW),
        Err(BindingError::Username(_))
    ));
    assert!(Identity::from_seed(&SEED)
        .sign_binding("a@b@c", &main_domain(), ISSUED)
        .is_err());
    assert!(matches!(
        binding_from_base64url(&"A".repeat(8193)),
        Err(BindingError::Malformed(_))
    ));
}

#[test]
fn a_legacy_name_can_be_bound() {
    let identity = Identity::from_seed(&SEED);
    let blob = identity
        .sign_binding("john.doe", &main_domain(), ISSUED)
        .unwrap();
    assert!(verify_binding(blob.as_bytes(), "john.doe", &accepted(), NOW).is_ok());
}

#[test]
fn a_signature_over_the_fields_without_the_binding_domain_is_refused() {
    let forged = with_signature(blob(), &raw_sign(&fields()));
    assert!(matches!(
        verify_binding(&forged, &alice(), &accepted(), NOW),
        Err(BindingError::Signature(_))
    ));
}

#[test]
fn an_app_signature_by_the_same_key_is_never_a_binding() {
    let digests = citadel_app_signature::ArtifactDigests::of(b"{}", b"\0asm");
    let app_key = citadel_app_signature::AppSigningKey::from_seed(&hmac_xi());
    assert_eq!(
        app_key.public_key_hex(),
        hex_of(Identity::from_seed(&SEED).pk())
    );
    let app_sig = app_key.sign("com.acme.kanban", "1.4.0", &digests).unwrap();
    let sig_bytes: Vec<u8> = (0..app_sig.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&app_sig[i..i + 2], 16).unwrap())
        .collect();
    let forged = with_signature(blob(), &sig_bytes);
    assert!(matches!(
        verify_binding(&forged, &alice(), &accepted(), NOW),
        Err(BindingError::Signature(_))
    ));
    let app_shaped = [citadel_app_signature::DOMAIN, &fields()].concat();
    let forged = with_signature(blob(), &raw_sign(&app_shaped));
    assert!(verify_binding(&forged, &alice(), &accepted(), NOW).is_err());
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

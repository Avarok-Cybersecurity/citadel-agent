use super::*;

// Fixture seeds: fixed so a failure is reproducible. Neither is, or derives, a release key.
const SEED: [u8; SEED_LEN] = [7; SEED_LEN];
const OTHER_SEED: [u8; SEED_LEN] = [9; SEED_LEN];
const TAG: &str = "agent-v0.9.0";
const NAME: &str = "Citadel-Agent.dmg";

fn signed() -> (String, [u8; 32], String) {
    let key = ReleaseSigningKey::from_seed(&SEED);
    let digest = sha256(b"the asset");
    let signature = key.sign(TAG, NAME, &digest).unwrap();
    (key.public_key_hex(), digest, signature)
}

#[test]
fn the_message_is_the_domain_tag_name_and_raw_digest_nul_separated() {
    let digest = [0xab; 32];
    let message = signed_message("agent-v1.2.3", "a.tar.gz", &digest).unwrap();
    let mut expected = b"citadel-agent-release-v1\0agent-v1.2.3\0a.tar.gz\0".to_vec();
    expected.extend_from_slice(&digest);
    assert_eq!(message, expected);
}

#[test]
fn a_nul_or_empty_field_cannot_be_signed_or_verified() {
    let digest = [0; 32];
    for (tag, name) in [("a\0b", NAME), (TAG, "x\0y"), ("", NAME), (TAG, "")] {
        assert!(matches!(
            signed_message(tag, name, &digest),
            Err(Refusal::BadField(_))
        ));
    }
}

#[test]
fn a_valid_signature_verifies_and_signing_is_deterministic() {
    let (key, digest, signature) = signed();
    assert_eq!(key.len(), PUBLIC_KEY_LEN * 2);
    assert_eq!(signature.len(), SIGNATURE_LEN * 2);
    assert_eq!(verify(&key, TAG, NAME, &digest, &signature), Ok(()));
    // The file's trailing newline and the key file's are ignored.
    assert_eq!(
        verify(
            &format!("{key}\n"),
            TAG,
            NAME,
            &digest,
            &format!("{signature}\n")
        ),
        Ok(())
    );
    let again = ReleaseSigningKey::from_seed(&SEED)
        .sign(TAG, NAME, &digest)
        .unwrap();
    assert_eq!(again, signature);
}

#[test]
fn a_tampered_asset_wrong_tag_or_wrong_name_is_invalid() {
    let (key, digest, signature) = signed();
    let tampered = sha256(b"the asset, changed");
    assert_eq!(
        verify(&key, TAG, NAME, &tampered, &signature),
        Err(Refusal::Invalid)
    );
    assert_eq!(
        verify(&key, "agent-v0.9.1", NAME, &digest, &signature),
        Err(Refusal::Invalid)
    );
    assert_eq!(
        verify(&key, TAG, "Citadel-Agent-x64.msi", &digest, &signature),
        Err(Refusal::Invalid)
    );
}

#[test]
fn another_keys_signature_is_invalid() {
    let (key, digest, _) = signed();
    let other = ReleaseSigningKey::from_seed(&OTHER_SEED);
    assert_ne!(other.public_key_hex(), key);
    let forged = other.sign(TAG, NAME, &digest).unwrap();
    assert_eq!(
        verify(&key, TAG, NAME, &digest, &forged),
        Err(Refusal::Invalid)
    );
    // Negative control: the same signature verifies under the key that made it.
    assert_eq!(
        verify(&other.public_key_hex(), TAG, NAME, &digest, &forged),
        Ok(())
    );
}

#[test]
fn a_truncated_empty_or_altered_signature_is_refused() {
    let (key, digest, signature) = signed();
    for bad in [&signature[..signature.len() - 2], "", "zz"] {
        assert!(matches!(
            verify(&key, TAG, NAME, &digest, bad),
            Err(Refusal::MalformedSignature(_))
        ));
    }
    let mut flipped = signature.into_bytes();
    flipped[100] = if flipped[100] == b'0' { b'1' } else { b'0' };
    let flipped = String::from_utf8(flipped).unwrap();
    assert!(verify(&key, TAG, NAME, &digest, &flipped).is_err());
}

#[test]
fn the_placeholder_key_refuses_every_signature() {
    let (_, digest, signature) = signed();
    assert_eq!(
        verify(PLACEHOLDER_PUBLIC_KEY, TAG, NAME, &digest, &signature),
        Err(Refusal::PlaceholderKey)
    );
    assert_eq!(
        parse_public_key(&format!("{PLACEHOLDER_PUBLIC_KEY}\n")).err(),
        Some(Refusal::PlaceholderKey)
    );
}

#[test]
fn a_malformed_public_key_or_seed_is_refused() {
    let (key, digest, signature) = signed();
    for bad in [&key[..key.len() - 2], "", "not hex"] {
        assert!(matches!(
            verify(bad, TAG, NAME, &digest, &signature),
            Err(Refusal::MalformedKey(_))
        ));
    }
    assert!(ReleaseSigningKey::from_seed_hex(&"0".repeat(63)).is_err());
    assert!(ReleaseSigningKey::from_seed_hex(&hex::encode(SEED)).is_ok());
}

#[test]
fn a_sha256_is_read_from_hex_only_at_its_length() {
    let digest = sha256(b"x");
    assert_eq!(sha256_from_hex(&hex::encode(digest)).unwrap(), digest);
    assert_eq!(
        sha256_from_hex(&hex::encode(digest).to_uppercase()).unwrap(),
        digest
    );
    assert!(sha256_from_hex(&hex::encode(digest)[..62]).is_err());
}

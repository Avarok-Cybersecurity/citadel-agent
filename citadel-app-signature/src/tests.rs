use super::*;

// Fixture seeds: fixed so a failure is reproducible. Neither is, or derives, a real key.
const SEED: [u8; SEED_LEN] = [7; SEED_LEN];
const OTHER_SEED: [u8; SEED_LEN] = [9; SEED_LEN];
const ID: &str = "com.acme.kanban";
const VERSION: &str = "1.4.0";
const MANIFEST: &[u8] = br#"{"format":1,"id":"com.acme.kanban"}"#;
const WASM: &[u8] = b"\0asm\x01\0\0\0";

fn signed() -> (String, ArtifactDigests, String) {
    let key = AppSigningKey::from_seed(&SEED);
    let digests = ArtifactDigests::of(MANIFEST, WASM);
    let signature = key.sign(ID, VERSION, &digests).unwrap();
    (key.public_key_hex(), digests, signature)
}

#[test]
fn the_message_is_the_domain_id_version_and_both_raw_digests() {
    let digests = ArtifactDigests {
        manifest: [0xab; 32],
        wasm: [0xcd; 32],
    };
    let message = signed_message("a.b", "1.2.3", &digests).unwrap();
    let mut expected = b"citadel-app-v1\0a.b\x001.2.3\0".to_vec();
    expected.extend_from_slice(&[0xab; 32]);
    expected.extend_from_slice(&[0xcd; 32]);
    assert_eq!(message, expected);
    assert_eq!(
        ArtifactDigests::of(MANIFEST, WASM).manifest,
        sha256(MANIFEST)
    );
    assert_eq!(ArtifactDigests::of(MANIFEST, WASM).wasm, sha256(WASM));
}

#[test]
fn a_malformed_id_or_version_cannot_be_signed_or_verified() {
    let digests = ArtifactDigests::of(MANIFEST, WASM);
    let key = AppSigningKey::from_seed(&SEED);
    for (id, version) in [
        ("com.acme\0.x", VERSION),
        ("Kanban", VERSION),
        ("", VERSION),
        (ID, "1.4"),
        (ID, "1.4.0\0"),
        (ID, "01.4.0"),
        (ID, ""),
    ] {
        assert!(matches!(
            signed_message(id, version, &digests),
            Err(Refusal::BadField(_))
        ));
        assert!(matches!(
            key.sign(id, version, &digests),
            Err(Refusal::BadField(_))
        ));
    }
}

#[test]
fn a_valid_signature_verifies_and_signing_is_deterministic() {
    let (key, digests, signature) = signed();
    assert_eq!(key.len(), PUBLIC_KEY_LEN * 2);
    assert_eq!(signature.len(), SIGNATURE_LEN * 2);
    assert_eq!(verify(&key, ID, VERSION, &digests, &signature), Ok(()));
    assert_eq!(
        verify(
            &format!("{key}\n"),
            ID,
            VERSION,
            &digests,
            &format!("{signature}\n")
        ),
        Ok(())
    );
    let again = AppSigningKey::from_seed(&SEED)
        .sign(ID, VERSION, &digests)
        .unwrap();
    assert_eq!(again, signature);
}

#[test]
fn another_keys_signature_is_invalid() {
    let (key, digests, _) = signed();
    let other = AppSigningKey::from_seed(&OTHER_SEED);
    assert_ne!(other.public_key_hex(), key);
    let forged = other.sign(ID, VERSION, &digests).unwrap();
    assert_eq!(
        verify(&key, ID, VERSION, &digests, &forged),
        Err(Refusal::Invalid)
    );
    // Control: the same signature verifies under the key that made it.
    assert_eq!(
        verify(&other.public_key_hex(), ID, VERSION, &digests, &forged),
        Ok(())
    );
}

#[test]
fn a_tampered_manifest_or_wasm_is_invalid() {
    let (key, digests, signature) = signed();
    let manifest = ArtifactDigests::of(br#"{"format":1,"id":"com.acme.kanban "}"#, WASM);
    let wasm = ArtifactDigests::of(MANIFEST, b"\0asm\x01\0\0\x01");
    let swapped = ArtifactDigests {
        manifest: digests.wasm,
        wasm: digests.manifest,
    };
    for tampered in [manifest, wasm, swapped] {
        assert_ne!(tampered, digests);
        assert_eq!(
            verify(&key, ID, VERSION, &tampered, &signature),
            Err(Refusal::Invalid)
        );
    }
}

#[test]
fn another_id_or_version_is_invalid() {
    let (key, digests, signature) = signed();
    for (id, version) in [
        ("com.acme.kanban2", VERSION),
        ("com.evil.kanban", VERSION),
        (ID, "1.3.2"),
        (ID, "1.4.1"),
        (ID, "14.0.0"),
    ] {
        assert_eq!(
            verify(&key, id, version, &digests, &signature),
            Err(Refusal::Invalid),
            "{id} {version}"
        );
    }
}

#[test]
fn an_agent_release_signature_by_the_same_key_is_not_an_app_signature() {
    use citadel_release_signature::{verify as verify_release, ReleaseSigningKey};
    let (key, digests, _) = signed();
    // The same seed, signing the fields an attacker would line up: tag = id, name = version.
    let release = ReleaseSigningKey::from_seed(&SEED);
    assert_eq!(release.public_key_hex(), key, "one key, two domains");
    let release_signature = release.sign(ID, VERSION, &digests.manifest).unwrap();
    assert_eq!(
        verify(&key, ID, VERSION, &digests, &release_signature),
        Err(Refusal::Invalid)
    );
    // Control: it is a perfectly good release signature, so only the domain refused it.
    assert_eq!(
        verify_release(&key, ID, VERSION, &digests.manifest, &release_signature),
        Ok(())
    );
}

#[test]
fn a_signature_over_the_message_under_another_domain_is_invalid() {
    let (key, digests, _) = signed();
    let mut xi = Seed::default();
    xi.copy_from_slice(&SEED);
    let raw = SigningKey::<MlDsa65>::from_seed(&xi);
    let body = &signed_message(ID, VERSION, &digests).unwrap()[DOMAIN.len()..];
    // The support-read domain, the release domain, and no domain at all.
    for domain in [
        &b"citadel-support-read-v1\0"[..],
        b"citadel-agent-release-v1\0",
        b"",
    ] {
        let message = [domain, body].concat();
        let signature: Signature<MlDsa65> = raw.sign(&message);
        let text = hex::encode(signature.encode().as_slice());
        assert_eq!(
            verify(&key, ID, VERSION, &digests, &text),
            Err(Refusal::Invalid)
        );
    }
    // Control: the same raw key over the app domain is accepted.
    let signature: Signature<MlDsa65> = raw.sign(&[DOMAIN, body].concat());
    let text = hex::encode(signature.encode().as_slice());
    assert_eq!(verify(&key, ID, VERSION, &digests, &text), Ok(()));
}

#[test]
fn a_truncated_empty_or_altered_signature_is_refused() {
    let (key, digests, signature) = signed();
    for bad in [&signature[..signature.len() - 2], "", "zz"] {
        assert!(matches!(
            verify(&key, ID, VERSION, &digests, bad),
            Err(Refusal::MalformedSignature(_))
        ));
    }
    let mut flipped = signature.into_bytes();
    flipped[100] = if flipped[100] == b'0' { b'1' } else { b'0' };
    let flipped = String::from_utf8(flipped).unwrap();
    assert!(verify(&key, ID, VERSION, &digests, &flipped).is_err());
}

#[test]
fn a_malformed_public_key_or_seed_is_refused() {
    let (key, digests, signature) = signed();
    for bad in [&key[..key.len() - 2], "", "not hex"] {
        assert!(matches!(
            verify(bad, ID, VERSION, &digests, &signature),
            Err(Refusal::MalformedKey(_))
        ));
        assert!(matches!(fingerprint(bad), Err(Refusal::MalformedKey(_))));
    }
    assert!(AppSigningKey::from_seed_hex(&"0".repeat(63)).is_err());
    assert!(AppSigningKey::from_seed_hex(&hex::encode(SEED)).is_ok());
}

#[test]
fn a_fingerprint_is_sixteen_hex_of_the_key_digest_in_fours() {
    let (key, _, _) = signed();
    let printed = fingerprint(&key).unwrap();
    let digest = hex::encode(sha256(&hex::decode(&key).unwrap()));
    assert_eq!(printed.replace(' ', ""), digest[..FINGERPRINT_HEX_CHARS]);
    assert_eq!(
        printed.split(' ').map(str::len).collect::<Vec<_>>(),
        [4, 4, 4, 4]
    );
    let other = fingerprint(&AppSigningKey::from_seed(&OTHER_SEED).public_key_hex()).unwrap();
    assert_ne!(printed, other);
}

#[test]
fn a_sha256_is_read_from_hex_only_at_its_length() {
    let digest = sha256(b"x");
    assert_eq!(sha256_from_hex(&hex::encode(digest)).unwrap(), digest);
    assert!(sha256_from_hex(&hex::encode(digest)[..62]).is_err());
}

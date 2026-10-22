//! The export against the crate's shared signed-app fixture: the valid signature is accepted,
//! and a tampered artifact, a tampered manifest, another publisher's key, a truncated signature
//! and a signature under another domain are each refused.

use super::verify_app_signature;
use serde_json::Value;

struct Case {
    key: String,
    other_key: String,
    id: String,
    version: String,
    manifest: String,
    wasm: String,
    signature: Vec<u8>,
    wrong_domain: Vec<u8>,
}

fn fixture() -> Case {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../citadel-app-signature/fixtures/signed-app.json"
    );
    let doc: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
    let text = |k: &str| doc[k].as_str().expect(k).to_string();
    let bytes = |k: &str| hex::decode(text(k)).expect(k);
    Case {
        key: text("public_key_hex"),
        other_key: text("other_public_key_hex"),
        id: text("app_id"),
        version: text("version"),
        manifest: text("manifest_sha256"),
        wasm: text("wasm_sha256"),
        signature: bytes("signature_hex"),
        wrong_domain: bytes("wrong_domain_signature_hex"),
    }
}

fn digest_of(bytes: &[u8]) -> String {
    hex::encode(citadel_app_signature::sha256(bytes))
}

fn check(c: &Case, key: &str, manifest: &str, wasm: &str, signature: &[u8]) -> bool {
    verify_app_signature(key, &c.id, &c.version, manifest, wasm, signature)
}

#[test]
fn the_fixture_signature_is_accepted() {
    let c = fixture();
    assert!(check(&c, &c.key, &c.manifest, &c.wasm, &c.signature));
}

#[test]
fn each_control_is_refused() {
    let c = fixture();
    let tampered = digest_of(b"\0asm\x01\0\0\x01");
    assert!(
        !check(&c, &c.key, &c.manifest, &tampered, &c.signature),
        "tampered artifact"
    );
    let manifest = digest_of(br#"{"format":1,"id":"com.acme.kanban","version":"1.4.1"}"#);
    assert!(
        !check(&c, &c.key, &manifest, &c.wasm, &c.signature),
        "tampered manifest"
    );
    assert!(
        !check(&c, &c.other_key, &c.manifest, &c.wasm, &c.signature),
        "another publisher"
    );
    let short = &c.signature[..c.signature.len() - 1];
    assert!(
        !check(&c, &c.key, &c.manifest, &c.wasm, short),
        "truncated signature"
    );
    assert!(
        !check(&c, &c.key, &c.manifest, &c.wasm, &c.wrong_domain),
        "another domain"
    );
    assert!(
        !check(&c, &c.key, "not hex", &c.wasm, &c.signature),
        "a malformed digest"
    );
}

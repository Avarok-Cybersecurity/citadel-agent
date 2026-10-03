//! Against the real attestation GitHub holds for agent-v0.8.6's Linux tarball, recorded in
//! tests/fixtures/updater, so the Sigstore path is exercised with no network.

use super::*;

const TARBALL_SHA: &str = "5f6950771bbd8acfacd9a11ac35cb40eb5efa98300f35d55691afa78c6187a67";
/// One attestation covers every asset of the release (it lists them all as subjects), so the
/// MSI's digest verifies too; a digest that is in no subject list must not. sha256("abc").
const MSI_SHA: &str = "4d4ee793f36b1b5a7129187f165a10f6e56c6ec82a019dba9e54aac6c18384ad";
const UNRELEASED_SHA: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

fn bundles() -> Vec<String> {
    let text = include_str!("../../tests/fixtures/updater/attestations-linux-tarball-0.8.6.json");
    let json: serde_json::Value = serde_json::from_str(text).unwrap();
    json["attestations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["bundle"].to_string())
        .collect()
}

#[test]
fn the_real_release_attestation_verifies_for_its_file() {
    let verifier = SigstoreVerifier::new().unwrap();
    assert_eq!(
        verifier.verify(TARBALL_SHA, &bundles(), "agent-v0.8.6"),
        Provenance::Verified
    );
}

#[test]
fn it_verifies_every_subject_and_nothing_else() {
    let verifier = SigstoreVerifier::new().unwrap();
    assert_eq!(
        verifier.verify(MSI_SHA, &bundles(), "agent-v0.8.6"),
        Provenance::Verified
    );
    assert!(matches!(
        verifier.verify(UNRELEASED_SHA, &bundles(), "agent-v0.8.6"),
        Provenance::Failed(_)
    ));
}

#[test]
fn an_old_attested_file_still_verifies_under_a_newer_tag() {
    // Signed from master, so the identity rule cannot tell releases apart: an old, attested
    // build re-uploaded as a newer release passes this check. What refuses it is the version
    // the staged file reports (verify::judge_staged; the app's CFBundleShortVersionString).
    let verifier = SigstoreVerifier::new().unwrap();
    assert_eq!(
        verifier.verify(TARBALL_SHA, &bundles(), "agent-v9.9.9"),
        Provenance::Verified
    );
}

#[test]
fn a_tampered_bundle_does_not_verify() {
    let verifier = SigstoreVerifier::new().unwrap();
    let tampered: Vec<String> = bundles()
        .into_iter()
        .map(|b| {
            let mut json: serde_json::Value = serde_json::from_str(&b).unwrap();
            let sig = json["dsseEnvelope"]["signatures"][0]["sig"]
                .as_str()
                .unwrap();
            let flipped = if sig.starts_with('M') { "N" } else { "M" };
            json["dsseEnvelope"]["signatures"][0]["sig"] =
                serde_json::Value::String(format!("{flipped}{}", &sig[1..]));
            json.to_string()
        })
        .collect();
    assert!(matches!(
        verifier.verify(TARBALL_SHA, &tampered, "agent-v0.8.6"),
        Provenance::Failed(_)
    ));
}

#[test]
fn no_bundles_is_absent_not_verified() {
    let verifier = SigstoreVerifier::new().unwrap();
    assert_eq!(
        verifier.verify(TARBALL_SHA, &[], "agent-v0.8.6"),
        Provenance::Absent
    );
}

#[test]
fn only_this_repositorys_release_workflow_is_trusted() {
    let base = "https://github.com/Avarok-Cybersecurity/citadel-workspace/.github/workflows";
    assert!(identity_allowed(
        &format!("{base}/release-agent.yml@refs/heads/master"),
        "agent-v0.9.0"
    ));
    assert!(identity_allowed(
        &format!("{base}/release-agent.yml@refs/tags/agent-v0.9.0"),
        "agent-v0.9.0"
    ));
    for bad in [
        format!("{base}/release-agent.yml@refs/tags/agent-v0.8.0"),
        format!("{base}/release-agent.yml@refs/heads/feature"),
        format!("{base}/validate.yml@refs/heads/master"),
        "https://github.com/someone/citadel-workspace/.github/workflows/release-agent.yml@refs/heads/master".to_string(),
        "https://github.com/Avarok-Cybersecurity/citadel-workspace-evil/.github/workflows/release-agent.yml@refs/heads/master".to_string(),
    ] {
        assert!(!identity_allowed(&bad, "agent-v0.9.0"), "{bad}");
    }
}

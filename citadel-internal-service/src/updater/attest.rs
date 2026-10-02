//! GitHub artifact attestations: Sigstore bundles verified against the production trust root
//! compiled into this crate, then held to this repository's release workflow.
//!
//! The root is embedded, not fetched by TUF, so the check needs no network beyond GitHub's. If
//! Sigstore rotates its keys before this agent is updated, verification fails closed: updates
//! are offered as downloads and not installed.

use super::io::AttestationVerifier;
use super::release::REPO;
use super::verify::Provenance;
use sigstore_trust_root::{TrustedRoot, SIGSTORE_PRODUCTION_TRUSTED_ROOT};
use sigstore_types::{Artifact, ArtifactDigest, Bundle, Sha256Hash};
use sigstore_verify::{SubjectAltName, VerificationPolicy, Verifier};

pub const GITHUB_ISSUER: &str = "https://token.actions.githubusercontent.com";
const WORKFLOW: &str = ".github/workflows/release-agent.yml";

/// Whether `identity`, a verified certificate's SAN, is this repository's release workflow run
/// from master (the deploy pipeline) or from the release's own tag (a tag push).
pub fn identity_allowed(identity: &str, tag: &str) -> bool {
    let workflow = format!("https://github.com/{REPO}/{WORKFLOW}@");
    match identity.strip_prefix(&workflow) {
        Some("refs/heads/master") => true,
        Some(git_ref) => git_ref.strip_prefix("refs/tags/") == Some(tag),
        None => false,
    }
}

pub struct SigstoreVerifier {
    verifier: Verifier,
}

impl SigstoreVerifier {
    pub fn new() -> Result<Self, String> {
        let root = TrustedRoot::from_json(SIGSTORE_PRODUCTION_TRUSTED_ROOT)
            .map_err(|e| format!("the embedded Sigstore trust root did not load: {e}"))?;
        let verifier = Verifier::new(&root)
            .map_err(|e| format!("the Sigstore verifier did not start: {e}"))?;
        Ok(Self { verifier })
    }

    fn verify_one(&self, digest: &Sha256Hash, bundle: &str, tag: &str) -> Result<(), String> {
        let bundle = Bundle::from_json(bundle).map_err(|e| format!("unreadable bundle: {e}"))?;
        let policy = VerificationPolicy::any_identity().require_issuer(GITHUB_ISSUER);
        let artifact = Artifact::from_digest(ArtifactDigest::sha256(*digest));
        let result = self
            .verifier
            .verify(artifact, &bundle, &policy)
            .map_err(|e| e.to_string())?;
        let identity = match result.identity() {
            Some(SubjectAltName::Uri(uri)) => uri.clone(),
            other => return Err(format!("the signer is {other:?}, not a workflow")),
        };
        if identity_allowed(&identity, tag) {
            Ok(())
        } else {
            Err(format!(
                "signed by {identity}, not this repository's release workflow"
            ))
        }
    }
}

impl AttestationVerifier for SigstoreVerifier {
    fn verify(&self, sha256: &str, bundles: &[String], tag: &str) -> Provenance {
        if bundles.is_empty() {
            return Provenance::Absent;
        }
        let digest = match Sha256Hash::from_hex(sha256) {
            Ok(digest) => digest,
            Err(e) => return Provenance::Failed(format!("{sha256} is not a sha256: {e}")),
        };
        let mut reasons = Vec::new();
        for bundle in bundles {
            match self.verify_one(&digest, bundle, tag) {
                Ok(()) => return Provenance::Verified,
                Err(why) => reasons.push(why),
            }
        }
        Provenance::Failed(reasons.join("; "))
    }
}

#[cfg(test)]
#[path = "attest_tests.rs"]
mod tests;

//! The ML-DSA-65 (FIPS 204) signature a publisher makes over a Citadel app: the one definition
//! of what is signed, shared by the publisher's signing tool, the workspace server (which
//! verifies before it stores an artifact), the browser wasm client and the native agent (which
//! verify again before instantiating). Verification is Rust everywhere, never JavaScript.
//!
//! # The signed message
//!
//! ```text
//! b"citadel-app-v1\0" || id || b"\0" || version || b"\0" || sha256(manifest bytes) || sha256(wasm)
//! ```
//!
//! `id` is the app's reverse-DNS id and `version` its `MAJOR.MINOR.PATCH`, both in the grammar
//! of `citadel-app-id` (so neither can hold a NUL, and the message is unambiguous). The two
//! digests are 32 raw bytes each: the manifest's exact published bytes and the wasm module's.
//! Binding the id and version means a signature cannot be moved onto another app, or replayed
//! onto an older release of the same one; binding both digests means neither file can change.
//! The domain differs from every other ML-DSA domain this codebase signs under (agent releases,
//! support reads), so no other signature by the same key is an app signature.
//!
//! The signature is deterministic, with an empty context string, published as
//! `app.mldsa.sig`: its 3309 bytes in lowercase hex, one line.
//!
//! # Keys
//!
//! The private key is ML-DSA's 32-byte seed (FIPS 204 `xi`), as 64 hex characters. The public
//! key is the 1952-byte encoded verifying key, as 3904 hex characters: the form a manifest's
//! `publisher.mldsa65_public_key` and an install's pinned key take.

use citadel_app_id::{validate_app_id, AppVersion};
use ml_dsa::signature::{Signer, Verifier};
use ml_dsa::VerifyingKey;
use ml_dsa::{EncodedSignature, EncodedVerifyingKey, MlDsa65, Seed, Signature, SigningKey};
use sha2::{Digest, Sha256};
use std::fmt;
use zeroize::{Zeroize, Zeroizing};

/// What every signed message starts with, so no other ML-DSA signature by the same key is one.
pub const DOMAIN: &[u8] = b"citadel-app-v1\0";
/// The detached signature's file name beside `manifest.json` and `app.wasm`.
pub const SIGNATURE_FILE_NAME: &str = "app.mldsa.sig";
pub const SEED_LEN: usize = 32;
pub const PUBLIC_KEY_LEN: usize = 1952;
pub const SIGNATURE_LEN: usize = 3309;
/// Hex characters of `sha256(public key)` a fingerprint shows.
pub const FINGERPRINT_HEX_CHARS: usize = 16;

/// Why a signature was not made or not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    MalformedKey(String),
    MalformedSignature(String),
    /// The id, version or a digest cannot be part of a message.
    BadField(String),
    /// Well formed, and not this key's signature over this id, version and these two files.
    Invalid,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedKey(why) => write!(f, "the publisher key is malformed: {why}"),
            Self::MalformedSignature(why) => write!(f, "the ML-DSA signature is malformed: {why}"),
            Self::BadField(why) => write!(f, "{why}"),
            Self::Invalid => write!(
                f,
                "the ML-DSA signature does not verify against the publisher key for this app, version, manifest and wasm"
            ),
        }
    }
}

impl std::error::Error for Refusal {}

/// The two files a signature covers, as their sha256 digests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactDigests {
    pub manifest: [u8; 32],
    pub wasm: [u8; 32],
}

impl ArtifactDigests {
    /// The digests of the manifest's exact bytes and the wasm module's.
    pub fn of(manifest: &[u8], wasm: &[u8]) -> Self {
        Self {
            manifest: sha256(manifest),
            wasm: sha256(wasm),
        }
    }
}

/// The bytes signed for release `version` of app `id` whose files have `digests`.
pub fn signed_message(
    id: &str,
    version: &str,
    digests: &ArtifactDigests,
) -> Result<Vec<u8>, Refusal> {
    validate_app_id(id).map_err(|why| Refusal::BadField(why.to_string()))?;
    AppVersion::parse(version).map_err(|why| Refusal::BadField(why.to_string()))?;
    let mut message = Vec::with_capacity(DOMAIN.len() + id.len() + version.len() + 66);
    message.extend_from_slice(DOMAIN);
    message.extend_from_slice(id.as_bytes());
    message.push(0);
    message.extend_from_slice(version.as_bytes());
    message.push(0);
    message.extend_from_slice(&digests.manifest);
    message.extend_from_slice(&digests.wasm);
    Ok(message)
}

/// The sha256 of `bytes`.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// A 64-character hex digest as bytes.
pub fn sha256_from_hex(text: &str) -> Result<[u8; 32], Refusal> {
    decode_exact::<32>(text.trim())
        .map_err(|why| Refusal::BadField(format!("{text:?} is not a sha256: {why}")))
}

/// A publisher's signing key: ML-DSA-65 expanded from its seed, wiped on drop.
pub struct AppSigningKey(SigningKey<MlDsa65>);

impl AppSigningKey {
    pub fn from_seed(seed: &[u8; SEED_LEN]) -> Self {
        let mut xi = Seed::default();
        xi.copy_from_slice(seed);
        let key = SigningKey::<MlDsa65>::from_seed(&xi);
        xi.as_mut_slice().zeroize();
        Self(key)
    }

    /// The private key file's contents: 64 hex characters, surrounding whitespace ignored.
    pub fn from_seed_hex(text: &str) -> Result<Self, Refusal> {
        let seed = Zeroizing::new(
            decode_exact::<SEED_LEN>(text.trim())
                .map_err(|why| Refusal::MalformedKey(format!("the private key: {why}")))?,
        );
        Ok(Self::from_seed(&seed))
    }

    pub fn public_key_hex(&self) -> String {
        use ml_dsa::signature::Keypair;
        hex::encode(self.0.verifying_key().encode().as_slice())
    }

    /// The `app.mldsa.sig` file's contents (without its newline).
    pub fn sign(
        &self,
        id: &str,
        version: &str,
        digests: &ArtifactDigests,
    ) -> Result<String, Refusal> {
        let message = signed_message(id, version, digests)?;
        let signature: Signature<MlDsa65> = self.0.sign(&message);
        Ok(hex::encode(signature.encode().as_slice()))
    }
}

/// Accepts only a valid ML-DSA-65 signature by `public_key` (hex) over
/// `signed_message(id, version, digests)`. `signature` is the `app.mldsa.sig` file's text.
pub fn verify(
    public_key: &str,
    id: &str,
    version: &str,
    digests: &ArtifactDigests,
    signature: &str,
) -> Result<(), Refusal> {
    let key = parse_public_key(public_key)?;
    let message = signed_message(id, version, digests)?;
    let bytes =
        decode_exact::<SIGNATURE_LEN>(signature.trim()).map_err(Refusal::MalformedSignature)?;
    let encoded = EncodedSignature::<MlDsa65>::try_from(&bytes[..])
        .map_err(|_| Refusal::MalformedSignature("not an ML-DSA-65 signature".to_string()))?;
    let signature = Signature::<MlDsa65>::decode(&encoded)
        .ok_or_else(|| Refusal::MalformedSignature("it does not decode".to_string()))?;
    key.verify(&message, &signature)
        .map_err(|_| Refusal::Invalid)
}

/// A publisher key, refusing anything not 1952 bytes of hex.
pub fn parse_public_key(text: &str) -> Result<VerifyingKey<MlDsa65>, Refusal> {
    let bytes = decode_exact::<PUBLIC_KEY_LEN>(text.trim()).map_err(Refusal::MalformedKey)?;
    let encoded = EncodedVerifyingKey::<MlDsa65>::try_from(&bytes[..])
        .map_err(|_| Refusal::MalformedKey("not an ML-DSA-65 key".to_string()))?;
    Ok(VerifyingKey::<MlDsa65>::decode(&encoded))
}

/// What the consent screen shows for a publisher key: the first 16 hex characters of the
/// sha256 of its 1952 bytes, in groups of four (`3f2a 9c01 77de b450`).
pub fn fingerprint(public_key: &str) -> Result<String, Refusal> {
    let bytes = decode_exact::<PUBLIC_KEY_LEN>(public_key.trim()).map_err(Refusal::MalformedKey)?;
    let digest = hex::encode(sha256(&bytes));
    let groups: Vec<&str> = (0..FINGERPRINT_HEX_CHARS)
        .step_by(4)
        .map(|at| &digest[at..at + 4])
        .collect();
    Ok(groups.join(" "))
}

fn decode_exact<const N: usize>(text: &str) -> Result<[u8; N], String> {
    if text.len() != N * 2 {
        return Err(format!(
            "{} hex characters, not the {} of {N} bytes",
            text.len(),
            N * 2
        ));
    }
    let mut out = [0u8; N];
    hex::decode_to_slice(text, &mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod fixture_tests;
#[cfg(test)]
mod tests;

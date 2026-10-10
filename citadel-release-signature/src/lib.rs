//! The ML-DSA-65 (FIPS 204) signature on a Citadel agent release asset: the one definition of
//! what is signed, shared by the release workflow's signing tool (citadel-workspace
//! `tools/release-sign`) and the agent's updater, which refuses an update without it.
//!
//! # The signed message
//!
//! ```text
//! b"citadel-agent-release-v1\0" || tag || b"\0" || asset_name || b"\0" || sha256(asset)
//! ```
//!
//! `tag` is the release tag (`agent-vX.Y.Z`), `asset_name` the file's name in the release
//! (`Citadel-Agent.dmg`), both UTF-8 and neither containing NUL; `sha256(asset)` is the 32 raw
//! bytes of the file's digest. Binding the tag and the name means a signature cannot be moved onto
//! another asset, or onto the same bytes published under another release. The signature is
//! ML-DSA-65 with an empty context string, deterministic, and published beside the asset as
//! `<asset_name>.mldsa.sig`: its 3309 bytes in lowercase hex, one line.
//!
//! # Keys
//!
//! The private key is ML-DSA's 32-byte seed (FIPS 204 `xi`), stored as 64 hex characters. The
//! public key is the 1952-byte encoded verifying key, as 3904 hex characters.

use ml_dsa::signature::{Signer, Verifier};
use ml_dsa::VerifyingKey;
use ml_dsa::{EncodedSignature, EncodedVerifyingKey, MlDsa65, Seed, Signature, SigningKey};
use sha2::{Digest, Sha256};
use std::fmt;
use zeroize::{Zeroize, Zeroizing};

/// What every signed message starts with, so no other ML-DSA signature by the same key is one.
pub const DOMAIN: &[u8] = b"citadel-agent-release-v1\0";
/// The signature's file name is the asset's with this appended.
pub const SIGNATURE_SUFFIX: &str = ".mldsa.sig";
/// Stands in for the release public key until the owner generates one. Never a key: every
/// verification against it is refused, and the release workflow will not publish while it is
/// embedded.
pub const PLACEHOLDER_PUBLIC_KEY: &str =
    "PLACEHOLDER: no release key yet. Replace this line with the public key `release-sign keygen` prints.";
pub const SEED_LEN: usize = 32;
pub const PUBLIC_KEY_LEN: usize = 1952;
pub const SIGNATURE_LEN: usize = 3309;

/// Why a signature was not made or not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The embedded key is still `PLACEHOLDER_PUBLIC_KEY`.
    PlaceholderKey,
    MalformedKey(String),
    MalformedSignature(String),
    /// The tag, name or digest cannot be part of a message.
    BadField(String),
    /// Well formed, and not this key's signature over this tag, name and digest.
    Invalid,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlaceholderKey => {
                write!(f, "this build has no release public key (it is the placeholder)")
            }
            Self::MalformedKey(why) => write!(f, "the release public key is malformed: {why}"),
            Self::MalformedSignature(why) => write!(f, "the ML-DSA signature is malformed: {why}"),
            Self::BadField(why) => write!(f, "{why}"),
            Self::Invalid => write!(
                f,
                "the ML-DSA signature does not verify against the release key for this tag, file and digest"
            ),
        }
    }
}

impl std::error::Error for Refusal {}

/// The bytes signed for `asset_name` of release `tag`, whose sha256 is `sha256`.
pub fn signed_message(tag: &str, asset_name: &str, sha256: &[u8; 32]) -> Result<Vec<u8>, Refusal> {
    for (what, value) in [("tag", tag), ("asset name", asset_name)] {
        if value.is_empty() || value.contains('\0') {
            return Err(Refusal::BadField(format!(
                "the {what} {value:?} is empty or contains NUL"
            )));
        }
    }
    let mut message = Vec::with_capacity(DOMAIN.len() + tag.len() + asset_name.len() + 34);
    message.extend_from_slice(DOMAIN);
    message.extend_from_slice(tag.as_bytes());
    message.push(0);
    message.extend_from_slice(asset_name.as_bytes());
    message.push(0);
    message.extend_from_slice(sha256);
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

/// The release signing key: ML-DSA-65 expanded from its seed, wiped on drop.
pub struct ReleaseSigningKey(SigningKey<MlDsa65>);

impl ReleaseSigningKey {
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

    /// The `.mldsa.sig` file's contents (without its newline) for `asset_name` of `tag`.
    pub fn sign(&self, tag: &str, asset_name: &str, sha256: &[u8; 32]) -> Result<String, Refusal> {
        let message = signed_message(tag, asset_name, sha256)?;
        let signature: Signature<MlDsa65> = self.0.sign(&message);
        Ok(hex::encode(signature.encode().as_slice()))
    }
}

/// Accepts only a valid ML-DSA-65 signature by `public_key` (hex, as embedded) over
/// `signed_message(tag, asset_name, sha256)`. `signature` is the `.mldsa.sig` file's text.
pub fn verify(
    public_key: &str,
    tag: &str,
    asset_name: &str,
    sha256: &[u8; 32],
    signature: &str,
) -> Result<(), Refusal> {
    let key = parse_public_key(public_key)?;
    let message = signed_message(tag, asset_name, sha256)?;
    let bytes =
        decode_exact::<SIGNATURE_LEN>(signature.trim()).map_err(Refusal::MalformedSignature)?;
    let encoded = EncodedSignature::<MlDsa65>::try_from(&bytes[..])
        .map_err(|_| Refusal::MalformedSignature("not an ML-DSA-65 signature".to_string()))?;
    let signature = Signature::<MlDsa65>::decode(&encoded)
        .ok_or_else(|| Refusal::MalformedSignature("it does not decode".to_string()))?;
    key.verify(&message, &signature)
        .map_err(|_| Refusal::Invalid)
}

/// The embedded public key, refusing the placeholder and anything not 1952 bytes of hex.
pub fn parse_public_key(text: &str) -> Result<VerifyingKey<MlDsa65>, Refusal> {
    let text = text.trim();
    if text == PLACEHOLDER_PUBLIC_KEY.trim() || text.starts_with("PLACEHOLDER") {
        return Err(Refusal::PlaceholderKey);
    }
    let bytes = decode_exact::<PUBLIC_KEY_LEN>(text).map_err(Refusal::MalformedKey)?;
    let encoded = EncodedVerifyingKey::<MlDsa65>::try_from(&bytes[..])
        .map_err(|_| Refusal::MalformedKey("not an ML-DSA-65 key".to_string()))?;
    Ok(VerifyingKey::<MlDsa65>::decode(&encoded))
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
mod tests;

//! The page's ML-DSA-65 check of a Citadel app before it is instantiated: a thin export of
//! `citadel_app_signature::verify`, so the signature scheme stays Rust everywhere and never
//! JavaScript. The page hashes (WebCrypto) and decides (`verify-artifact.ts`); this only answers
//! whether `signature` is the publisher key's signature over the app-v1 message for this id,
//! version and pair of digests. The key passed is the one the install pinned, never the
//! manifest's own claim, so the pinned key stays the trust anchor.

use citadel_app_signature::{ArtifactDigests, sha256_from_hex, verify};
use wasm_bindgen::prelude::*;

/// True only for a valid signature. `signature` is the `app.mldsa.sig` bytes (its hex decoded);
/// the digests are lowercase hex sha256. Anything malformed is false, never an exception.
#[wasm_bindgen]
pub fn verify_app_signature(
    public_key_hex: &str,
    app_id: &str,
    version: &str,
    manifest_sha256_hex: &str,
    wasm_sha256_hex: &str,
    signature: &[u8],
) -> bool {
    let (Ok(manifest), Ok(wasm)) = (
        sha256_from_hex(manifest_sha256_hex),
        sha256_from_hex(wasm_sha256_hex),
    ) else {
        return false;
    };
    let digests = ArtifactDigests { manifest, wasm };
    verify(
        public_key_hex,
        app_id,
        version,
        &digests,
        &hex::encode(signature),
    )
    .is_ok()
}

#[cfg(test)]
mod tests;

//! The bytes an op's and a claim's signatures cover, and their ML-DSA-65 verification
//! (`vfs-signing.md` §1, `vfs-outsiders.md` §3). Pure ML-DSA, empty context, as `citadel-identity`.

use crate::hash::lp;
use citadel_identity::{MlDsa65Pk, SignatureError, SIGNATURE_LEN};
use ml_dsa::signature::Verifier as _;
use ml_dsa::{EncodedSignature, EncodedVerifyingKey, MlDsa65, Signature, VerifyingKey};

pub const VFS_OP_DOMAIN: &[u8] = b"citadel-vfs-op-v1";
pub const VFS_CLAIM_DOMAIN: &[u8] = b"citadel-vfs-claim-v1";

fn domain_separated(domain: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let mut message =
        Vec::with_capacity(domain.len() + 1 + parts.iter().map(|p| p.len()).sum::<usize>());
    message.extend_from_slice(domain);
    message.push(0);
    for part in parts {
        message.extend_from_slice(part);
    }
    message
}

/// `"citadel-vfs-op-v1" ‖ 0x00 ‖ header`: what an op's signature covers.
pub fn op_message(header: &[u8]) -> Vec<u8> {
    domain_separated(VFS_OP_DOMAIN, &[header])
}

/// `"citadel-vfs-claim-v1" ‖ 0x00 ‖ lp(volume) ‖ target ‖ key_pk`; `None` if `volume` cannot frame.
pub fn claim_message(volume: &str, target: &[u8; 16], key_pk: &[u8]) -> Option<Vec<u8>> {
    let mut framed = Vec::new();
    lp(&mut framed, volume).ok()?;
    Some(domain_separated(
        VFS_CLAIM_DOMAIN,
        &[&framed, target, key_pk],
    ))
}

/// An op's signature by `pk` over its header bytes exactly as carried.
pub fn verify_op(pk: &MlDsa65Pk, header: &[u8], sig: &[u8]) -> Result<(), SignatureError> {
    verify(pk, &op_message(header), sig)
}

/// A claim's `link_sig` by the link key `link_pk` over `key_pk` for this volume and entry.
pub fn verify_claim(
    link_pk: &MlDsa65Pk,
    volume: &str,
    target: &[u8; 16],
    key_pk: &MlDsa65Pk,
    sig: &[u8],
) -> Result<(), SignatureError> {
    let message = claim_message(volume, target, key_pk.as_bytes())
        .ok_or_else(|| SignatureError::MalformedKey("volume too long".to_string()))?;
    verify(link_pk, &message, sig)
}

fn verify(pk: &MlDsa65Pk, message: &[u8], signature: &[u8]) -> Result<(), SignatureError> {
    let encoded = EncodedVerifyingKey::<MlDsa65>::try_from(&pk.as_bytes()[..])
        .map_err(|_| SignatureError::MalformedKey("not an ML-DSA-65 key".to_string()))?;
    let key = VerifyingKey::<MlDsa65>::decode(&encoded);
    let encoded = EncodedSignature::<MlDsa65>::try_from(signature).map_err(|_| {
        SignatureError::MalformedSignature(format!(
            "{} bytes, not {SIGNATURE_LEN}",
            signature.len()
        ))
    })?;
    let signature = Signature::<MlDsa65>::decode(&encoded)
        .ok_or_else(|| SignatureError::MalformedSignature("it does not decode".to_string()))?;
    key.verify(message, &signature)
        .map_err(|_| SignatureError::Invalid)
}

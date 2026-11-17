//! The sign-in identity proof (identity-api.md §7a, sign-in-with-citadel.md §1a): the person's
//! agent signs the issuer's attempt with the **identity key**, and the issuer verifies it under the
//! key carried inside the proof, so the proof is self-certifying and the issuer keeps no registry.
//!
//! ```text
//! body = str16(issuer) ‖ str16(aud) ‖ str16(origin) ‖ nonce[32] ‖ challenge[32] ‖ identity_pk[1952]
//! sig  = ML-DSA-65.Sign(sk_identity, "citadel-sign-in-proof-v1" ‖ 0x00 ‖ body)
//! blob = 0x01 ‖ body ‖ sig[3309]
//! ```
//!
//! `origin` is the browser `Origin` the agent saw on its control connection's handshake, never a
//! value the page sent; the issuer accepts only its own. No clock: freshness is the issuer's
//! single-use attempt, bound here by `nonce` and `challenge`.

use crate::codec::{domain_separated, put_str16, Reader};
use crate::identity::Identity;
use crate::keys::{verify, MlDsa65Pk, SignatureError, PUBLIC_KEY_LEN, SIGNATURE_LEN};
use crate::server::NONCE_LEN;
use std::fmt;

pub const PROOF_DOMAIN: &[u8] = b"citadel-sign-in-proof-v1";
pub const PROOF_VERSION: u8 = 0x01;
pub const MAX_PROOF_FIELD_BYTES: usize = 255;
/// The blob without its three strings: version, three length prefixes, nonce, challenge, key, sig.
pub const PROOF_FIXED_LEN: usize = 1 + 3 * 2 + 2 * NONCE_LEN + PUBLIC_KEY_LEN + SIGNATURE_LEN;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInProof {
    /// The issuer's canonical domain (`work.avarok.net`).
    pub issuer: String,
    /// The client the person consented to (`sw-mcp`).
    pub aud: String,
    /// The browser `Origin` of the agent's control connection.
    pub origin: String,
    /// The issuer's attempt nonce.
    pub nonce: [u8; NONCE_LEN],
    /// `SHA3-256(verifier)` from the link.
    pub challenge: [u8; NONCE_LEN],
    /// The subject: the identity whose key made the signature.
    pub identity: MlDsa65Pk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofError {
    Malformed(String),
    Version(u8),
    Field(&'static str),
    Signature(SignatureError),
    WrongIssuer { expected: String, found: String },
    WrongOrigin { expected: String, found: String },
    WrongAudience { expected: String, found: String },
    WrongNonce,
    WrongChallenge,
}

impl fmt::Display for ProofError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(why) => write!(f, "the sign-in proof is malformed: {why}"),
            Self::Version(v) => write!(
                f,
                "sign-in proof version {v:#04x} is not {PROOF_VERSION:#04x}"
            ),
            Self::Field(name) => write!(
                f,
                "the proof's {name} must be 1 to {MAX_PROOF_FIELD_BYTES} bytes"
            ),
            Self::Signature(why) => write!(f, "the sign-in proof: {why}"),
            Self::WrongIssuer { expected, found } => {
                write!(f, "the proof is for issuer {found:?}, not {expected:?}")
            }
            Self::WrongOrigin { expected, found } => {
                write!(f, "the proof was made on {found:?}, not {expected:?}")
            }
            Self::WrongAudience { expected, found } => {
                write!(f, "the proof is for client {found:?}, not {expected:?}")
            }
            Self::WrongNonce => write!(f, "the proof answers another sign-in attempt"),
            Self::WrongChallenge => write!(f, "the proof answers another sign-in link"),
        }
    }
}

impl std::error::Error for ProofError {}

fn body(p: &SignInProof) -> Result<Vec<u8>, ProofError> {
    let fields = [
        ("issuer", &p.issuer),
        ("aud", &p.aud),
        ("origin", &p.origin),
    ];
    let mut body =
        Vec::with_capacity(PROOF_FIXED_LEN + p.issuer.len() + p.aud.len() + p.origin.len());
    for (name, value) in fields {
        if value.is_empty() || value.len() > MAX_PROOF_FIELD_BYTES {
            return Err(ProofError::Field(name));
        }
        put_str16(&mut body, value).map_err(|_| ProofError::Field(name))?;
    }
    body.extend_from_slice(&p.nonce);
    body.extend_from_slice(&p.challenge);
    body.extend_from_slice(p.identity.as_bytes());
    Ok(body)
}

/// The bytes the proof's signature covers.
pub fn proof_message(p: &SignInProof) -> Result<Vec<u8>, ProofError> {
    Ok(domain_separated(PROOF_DOMAIN, &body(p)?))
}

impl Identity {
    /// This identity's proof for the issuer's attempt. `origin` must be the browser `Origin` the
    /// caller observed on the transport, never one a page supplied.
    pub fn sign_proof(
        &self,
        issuer: &str,
        aud: &str,
        origin: &str,
        nonce: &[u8; NONCE_LEN],
        challenge: &[u8; NONCE_LEN],
    ) -> Result<Vec<u8>, ProofError> {
        let proof = SignInProof {
            issuer: issuer.to_string(),
            aud: aud.to_string(),
            origin: origin.to_string(),
            nonce: *nonce,
            challenge: *challenge,
            identity: self.public_key().clone(),
        };
        let body = body(&proof)?;
        let sig = self.signer.sign(&domain_separated(PROOF_DOMAIN, &body));
        let mut blob = Vec::with_capacity(1 + body.len() + SIGNATURE_LEN);
        blob.push(PROOF_VERSION);
        blob.extend_from_slice(&body);
        blob.extend_from_slice(&sig);
        Ok(blob)
    }
}

/// Accepts `blob` only as a proof, signed by the key it carries, for this issuer, made on this
/// origin, for this client, answering this attempt's nonce and this link's challenge. Checks in
/// the contract's order: parse, version, signature, issuer, origin, aud, nonce, challenge.
pub fn verify_proof(
    blob: &[u8],
    expect_issuer: &str,
    expect_origin: &str,
    expect_aud: &str,
    expect_nonce: &[u8; NONCE_LEN],
    expect_challenge: &[u8; NONCE_LEN],
) -> Result<SignInProof, ProofError> {
    let proof = signed(blob)?;
    let mismatch = |expected: &str, found: &str| {
        (expected != found).then(|| (expected.to_string(), found.to_string()))
    };
    if let Some((expected, found)) = mismatch(expect_issuer, &proof.issuer) {
        return Err(ProofError::WrongIssuer { expected, found });
    }
    if let Some((expected, found)) = mismatch(expect_origin, &proof.origin) {
        return Err(ProofError::WrongOrigin { expected, found });
    }
    if let Some((expected, found)) = mismatch(expect_aud, &proof.aud) {
        return Err(ProofError::WrongAudience { expected, found });
    }
    if proof.nonce != *expect_nonce {
        return Err(ProofError::WrongNonce);
    }
    if proof.challenge != *expect_challenge {
        return Err(ProofError::WrongChallenge);
    }
    Ok(proof)
}

fn signed(blob: &[u8]) -> Result<SignInProof, ProofError> {
    let mut reader = Reader::new(blob);
    let version = reader.u8("version").map_err(ProofError::Malformed)?;
    if version != PROOF_VERSION {
        return Err(ProofError::Version(version));
    }
    let issuer = reader.str16("issuer").map_err(ProofError::Malformed)?;
    let aud = reader.str16("aud").map_err(ProofError::Malformed)?;
    let origin = reader.str16("origin").map_err(ProofError::Malformed)?;
    let nonce = reader
        .array::<NONCE_LEN>("nonce")
        .map_err(ProofError::Malformed)?;
    let challenge = reader
        .array::<NONCE_LEN>("challenge")
        .map_err(ProofError::Malformed)?;
    let identity = reader
        .take(PUBLIC_KEY_LEN, "identity")
        .map_err(ProofError::Malformed)?;
    let sig = reader
        .take(SIGNATURE_LEN, "signature")
        .map_err(ProofError::Malformed)?;
    reader.finish().map_err(ProofError::Malformed)?;

    let proof = SignInProof {
        issuer: issuer.to_string(),
        aud: aud.to_string(),
        origin: origin.to_string(),
        nonce,
        challenge,
        identity: MlDsa65Pk::from_bytes(identity).map_err(ProofError::Signature)?,
    };
    verify(&proof.identity, &proof_message(&proof)?, sig).map_err(ProofError::Signature)?;
    Ok(proof)
}

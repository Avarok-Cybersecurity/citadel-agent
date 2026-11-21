//! The server's ML-DSA-65 identity key and the post-connect `ServerIdentityChallenge` (identity
//! doc §3.1 rev b). The kernel signs, the agent verifies against the key it pinned.
//!
//! ```text
//! sig = ML-DSA-65.Sign(sk_server, "citadel-server-challenge-v1" ‖ 0x00 ‖ nonce[32] ‖ u64be(cid) ‖ utf8(domain))
//! ```
//!
//! Binding the CID and the domain means an answer given to one session or under one name is
//! not an answer for another.

use crate::codec::{domain_separated, hmac_sha3_256};
use crate::domain::CanonicalDomain;
use crate::keys::{verify, MlDsa65Pk, SignatureError, Signer, SEED_LEN};
use std::fmt;
use zeroize::Zeroizing;

pub const SERVER_KEY_LABEL: &[u8] = b"citadel-server-sign-v1";
pub const CHALLENGE_DOMAIN: &[u8] = b"citadel-server-challenge-v1";
pub const NONCE_LEN: usize = 32;

/// A server's signing key, expanded from the 32-byte seed the kernel keeps sealed:
/// `xi = HMAC-SHA3-256(seed, "citadel-server-sign-v1")`. Wiped on drop.
pub struct ServerKey {
    pub(crate) signer: Signer,
}

impl fmt::Debug for ServerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ServerKey({:?})", self.signer.pk())
    }
}

impl ServerKey {
    pub fn from_seed(seed: &[u8; SEED_LEN]) -> ServerKey {
        let xi = Zeroizing::new(hmac_sha3_256(seed, &[SERVER_KEY_LABEL]));
        ServerKey {
            signer: Signer::from_xi(&xi),
        }
    }

    pub fn public_key(&self) -> &MlDsa65Pk {
        self.signer.pk()
    }

    /// The answer to a `ServerIdentityChallenge` from session `cid`.
    pub fn sign_challenge(
        &self,
        domain: &CanonicalDomain,
        cid: u64,
        nonce: &[u8; NONCE_LEN],
    ) -> Vec<u8> {
        self.signer.sign(&challenge_message(domain, cid, nonce))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChallengeError {
    Signature(SignatureError),
}

impl fmt::Display for ChallengeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Signature(why) => write!(f, "the server's identity challenge answer: {why}"),
        }
    }
}

impl std::error::Error for ChallengeError {}

/// The signed message for `nonce` from session `cid` to the server named `domain`.
pub fn challenge_message(domain: &CanonicalDomain, cid: u64, nonce: &[u8; NONCE_LEN]) -> Vec<u8> {
    let mut body = Vec::with_capacity(NONCE_LEN + 8 + domain.as_str().len());
    body.extend_from_slice(nonce);
    body.extend_from_slice(&cid.to_be_bytes());
    body.extend_from_slice(domain.as_str().as_bytes());
    domain_separated(CHALLENGE_DOMAIN, &body)
}

/// Accepts only the pinned key's answer to this nonce, for this CID, under this domain.
pub fn verify_server_challenge(
    pk: &MlDsa65Pk,
    domain: &CanonicalDomain,
    cid: u64,
    nonce: &[u8; NONCE_LEN],
    sig: &[u8],
) -> Result<(), ChallengeError> {
    verify(pk, &challenge_message(domain, cid, nonce), sig).map_err(ChallengeError::Signature)
}

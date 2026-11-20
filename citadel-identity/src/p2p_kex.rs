//! The identity key's signature over a P2P key exchange (the SDK's `proto::peer::kex_auth`): each
//! end signs its stage's transcript, and the other end checks it against the key it pinned for
//! the person. The SDK builds the transcript body; this signs `"citadel-p2p-kex-v1" ‖ 0x00 ‖ body`.
//!
//! [`P2pKexSigner`] holds the signing key alone, not the password key, so a signed-in session
//! can keep it for as long as the session lasts without keeping what derives its passwords.

use crate::codec::domain_separated;
use crate::identity::Identity;
use crate::keys::{fingerprint, MlDsa65Pk, Signer};
use std::fmt;

/// The domain of every P2P key-exchange signature; the SDK's `KEX_TRANSCRIPT_DOMAIN`.
pub const P2P_KEX_DOMAIN: &[u8] = b"citadel-p2p-kex-v1";

/// The identity key, able to sign P2P key-exchange transcripts and nothing else.
#[derive(Clone)]
pub struct P2pKexSigner(Signer);

impl fmt::Debug for P2pKexSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "P2pKexSigner({})", fingerprint(self.0.pk()))
    }
}

impl P2pKexSigner {
    pub fn public_key(&self) -> &MlDsa65Pk {
        self.0.pk()
    }

    /// ML-DSA-65 (pure, empty context) over `P2P_KEX_DOMAIN ‖ 0x00 ‖ body`.
    pub fn sign(&self, body: &[u8]) -> Vec<u8> {
        self.0.sign(&domain_separated(P2P_KEX_DOMAIN, body))
    }
}

impl Identity {
    /// This identity's P2P key-exchange signer.
    pub fn p2p_kex_signer(&self) -> P2pKexSigner {
        P2pKexSigner(self.signer.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::verify;

    #[test]
    fn a_transcript_signature_is_made_under_its_own_domain_by_the_identity_key() {
        let identity = Identity::from_seed(&[7; 32]);
        let signer = identity.p2p_kex_signer();
        assert_eq!(signer.public_key(), identity.public_key());
        let signature = signer.sign(b"body");
        let message = domain_separated(P2P_KEX_DOMAIN, b"body");
        assert_eq!(verify(identity.public_key(), &message, &signature), Ok(()));
        assert!(verify(identity.public_key(), b"body", &signature).is_err());
        let other = domain_separated(crate::BINDING_DOMAIN, b"body");
        assert!(verify(identity.public_key(), &other, &signature).is_err());
    }
}

//! Test keys: ML-DSA-65 from fixed seeds (published test values, never production keys).

use citadel_identity::{fingerprint, MlDsa65Pk};
use citadel_vfs::op::{claim_message, op_message, Draft, SignedOp};
use ml_dsa::signature::{Keypair as _, Signer as _};
use ml_dsa::{MlDsa65, Seed, Signature, SigningKey};

pub struct Key {
    sk: SigningKey<MlDsa65>,
    pub pk: MlDsa65Pk,
    pub fp: String,
}

impl Key {
    pub fn from_byte(b: u8) -> Self {
        let mut seed = Seed::default();
        seed.copy_from_slice(&[b; 32]);
        let sk = SigningKey::<MlDsa65>::from_seed(&seed);
        let pk = MlDsa65Pk::from_bytes(sk.verifying_key().encode().as_slice())
            .unwrap_or_else(|e| panic!("{e:?}"));
        let fp = fingerprint(&pk);
        Self { sk, pk, fp }
    }

    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        let signature: Signature<MlDsa65> = self.sk.sign(message);
        signature.encode().as_slice().to_vec()
    }

    pub fn sign_op(&self, draft: &Draft) -> SignedOp {
        let (header, body) = draft.to_bytes();
        let sig = self.sign(&op_message(&header));
        SignedOp {
            header,
            body,
            key: self.fp.clone(),
            sig,
        }
    }

    /// This link key's `link_sig` over `guest` for `target` in `volume`.
    pub fn sign_claim(&self, volume: &str, target: &[u8; 16], guest: &Key) -> Vec<u8> {
        let message =
            claim_message(volume, target, guest.pk.as_bytes()).unwrap_or_else(|| panic!("frame"));
        self.sign(&message)
    }
}

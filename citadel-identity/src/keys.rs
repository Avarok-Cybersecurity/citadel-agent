//! ML-DSA-65 (FIPS 204) keys: the public key type every verifier takes, the deterministic
//! signer the identity and the server key wrap, and the one verify every format calls.

use crate::codec::{base32_lower_nopad, base64url_decode, base64url_encode, sha3_256};
use ml_dsa::signature::{Keypair, Signer as _, Verifier as _};
use ml_dsa::VerifyingKey;
use ml_dsa::{EncodedSignature, EncodedVerifyingKey, MlDsa65, Seed, Signature, SigningKey};
use std::fmt;
use zeroize::Zeroize;

pub const SEED_LEN: usize = 32;
pub const PUBLIC_KEY_LEN: usize = 1952;
pub const SIGNATURE_LEN: usize = 3309;

/// Domain of the identity fingerprint hash.
pub const FINGERPRINT_DOMAIN: &[u8] = b"citadel-identity-fp-v1";
/// Bytes of the fingerprint hash kept: 80 bits, 16 base32 characters.
pub const FINGERPRINT_BYTES: usize = 10;

/// An encoded ML-DSA-65 verifying key.
#[derive(Clone, PartialEq, Eq)]
pub struct MlDsa65Pk(Box<[u8; PUBLIC_KEY_LEN]>);

impl fmt::Debug for MlDsa65Pk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MlDsa65Pk({})", fingerprint(self))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureError {
    MalformedKey(String),
    MalformedSignature(String),
    /// Well formed, and not this key's signature over this message.
    Invalid,
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedKey(why) => write!(f, "the ML-DSA-65 key is malformed: {why}"),
            Self::MalformedSignature(why) => {
                write!(f, "the ML-DSA-65 signature is malformed: {why}")
            }
            Self::Invalid => write!(f, "the ML-DSA-65 signature does not verify"),
        }
    }
}

impl std::error::Error for SignatureError {}

impl MlDsa65Pk {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SignatureError> {
        let array: [u8; PUBLIC_KEY_LEN] = bytes.try_into().map_err(|_| {
            SignatureError::MalformedKey(format!("{} bytes, not {PUBLIC_KEY_LEN}", bytes.len()))
        })?;
        Ok(Self(Box::new(array)))
    }

    pub fn from_base64url(text: &str) -> Result<Self, SignatureError> {
        Self::from_bytes(&base64url_decode(text).map_err(SignatureError::MalformedKey)?)
    }

    pub fn as_bytes(&self) -> &[u8; PUBLIC_KEY_LEN] {
        &self.0
    }

    pub fn to_base64url(&self) -> String {
        base64url_encode(&self.0[..])
    }

    fn verifying_key(&self) -> Result<VerifyingKey<MlDsa65>, SignatureError> {
        let encoded = EncodedVerifyingKey::<MlDsa65>::try_from(&self.0[..])
            .map_err(|_| SignatureError::MalformedKey("not an ML-DSA-65 key".to_string()))?;
        Ok(VerifyingKey::<MlDsa65>::decode(&encoded))
    }
}

/// `base32_lower_nopad(SHA3-256("citadel-identity-fp-v1" ‖ 0x00 ‖ pk)[..10])`: 16 characters.
pub fn fingerprint(pk: &MlDsa65Pk) -> String {
    let digest = sha3_256(&[FINGERPRINT_DOMAIN, &[0], &pk.0[..]]);
    base32_lower_nopad(&digest[..FINGERPRINT_BYTES])
}

/// The fingerprint in groups of four for people to compare: `abcd-efgh-ijkl-mnop`.
pub fn fingerprint_grouped(pk: &MlDsa65Pk) -> String {
    let fp = fingerprint(pk);
    let groups: Vec<&str> = (0..fp.len()).step_by(4).map(|at| &fp[at..at + 4]).collect();
    groups.join("-")
}

/// A deterministic ML-DSA-65 signing key expanded from a 32-byte `xi`; wiped on drop.
pub(crate) struct Signer {
    key: SigningKey<MlDsa65>,
    pk: MlDsa65Pk,
}

impl Signer {
    pub(crate) fn from_xi(xi: &[u8; SEED_LEN]) -> Self {
        let mut seed = Seed::default();
        seed.copy_from_slice(xi);
        let key = SigningKey::<MlDsa65>::from_seed(&seed);
        seed.as_mut_slice().zeroize();
        let encoded = key.verifying_key().encode();
        let pk = MlDsa65Pk::from_bytes(encoded.as_slice())
            .unwrap_or_else(|_| unreachable!("an ML-DSA-65 verifying key encodes to 1952 bytes"));
        Self { key, pk }
    }

    pub(crate) fn pk(&self) -> &MlDsa65Pk {
        &self.pk
    }

    /// Pure ML-DSA, empty context. `message` must come from `codec::domain_separated`.
    pub(crate) fn sign(&self, message: &[u8]) -> Vec<u8> {
        let signature: Signature<MlDsa65> = self.key.sign(message);
        signature.encode().as_slice().to_vec()
    }
}

/// Accepts only a valid ML-DSA-65 signature by `pk` over `message`.
pub(crate) fn verify(
    pk: &MlDsa65Pk,
    message: &[u8],
    signature: &[u8],
) -> Result<(), SignatureError> {
    let key = pk.verifying_key()?;
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

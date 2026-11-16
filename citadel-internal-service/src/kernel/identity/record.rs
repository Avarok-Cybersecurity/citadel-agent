//! The agent's one identity record: the public key, its fingerprint and the sealed seed, as the
//! identity store keeps it (`IdentityStore`). Pure encoding; unknown fields are refused.

use super::seal::{KdfParams, SealedSeed, NONCE_LEN, SALT_LEN};
use citadel_identity::{base64url_decode, base64url_encode, fingerprint, MlDsa65Pk};
use serde::{Deserialize, Serialize};

const RECORD_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdentityRecord {
    pub(crate) pk: MlDsa65Pk,
    pub(crate) id_fp: String,
    pub(crate) created_at_s: u64,
    pub(crate) sealed: SealedSeed,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    v: u8,
    pk: String,
    id_fp: String,
    created_at_s: u64,
    kdf: KdfParams,
    salt: String,
    nonce: String,
    sealed_seed: String,
}

impl IdentityRecord {
    pub(crate) fn new(pk: MlDsa65Pk, created_at_s: u64, sealed: SealedSeed) -> Self {
        Self {
            id_fp: fingerprint(&pk),
            pk,
            created_at_s,
            sealed,
        }
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let wire = Wire {
            v: RECORD_VERSION,
            pk: self.pk.to_base64url(),
            id_fp: self.id_fp.clone(),
            created_at_s: self.created_at_s,
            kdf: self.sealed.kdf,
            salt: base64url_encode(&self.sealed.salt),
            nonce: base64url_encode(&self.sealed.nonce),
            sealed_seed: base64url_encode(&self.sealed.ciphertext),
        };
        serde_json::to_vec(&wire).unwrap_or_else(|_| unreachable!("the record is plain data"))
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, String> {
        let wire: Wire = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if wire.v != RECORD_VERSION {
            return Err(format!(
                "identity record version {} is not {RECORD_VERSION}",
                wire.v
            ));
        }
        let pk = MlDsa65Pk::from_base64url(&wire.pk).map_err(|e| e.to_string())?;
        if fingerprint(&pk) != wire.id_fp {
            return Err("the identity record's fingerprint is not its key's".to_string());
        }
        let salt: [u8; SALT_LEN] = base64url_decode(&wire.salt)?
            .try_into()
            .map_err(|_| "the salt is not 32 bytes".to_string())?;
        let nonce: [u8; NONCE_LEN] = base64url_decode(&wire.nonce)?
            .try_into()
            .map_err(|_| "the nonce is not 12 bytes".to_string())?;
        Ok(Self {
            pk,
            id_fp: wire.id_fp,
            created_at_s: wire.created_at_s,
            sealed: SealedSeed {
                kdf: wire.kdf,
                salt,
                nonce,
                ciphertext: base64url_decode(&wire.sealed_seed)?,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::identity::seal::MIN_PARAMS;
    use citadel_identity::Identity;

    fn record() -> IdentityRecord {
        let pk = Identity::from_seed(&[3; 32]).public_key().clone();
        let sealed = SealedSeed {
            kdf: MIN_PARAMS,
            salt: [1; 32],
            nonce: [2; 12],
            ciphertext: vec![9; 48],
        };
        IdentityRecord::new(pk, 1_794_700_800, sealed)
    }

    #[test]
    fn a_record_round_trips() {
        let r = record();
        assert_eq!(IdentityRecord::decode(&r.encode()).unwrap(), r);
    }

    #[test]
    fn a_swapped_fingerprint_or_an_unknown_field_is_refused() {
        let mut value: serde_json::Value = serde_json::from_slice(&record().encode()).unwrap();
        value["id_fp"] = "aaaaaaaaaaaaaaaa".into();
        assert!(IdentityRecord::decode(&serde_json::to_vec(&value).unwrap()).is_err());
        let mut value: serde_json::Value = serde_json::from_slice(&record().encode()).unwrap();
        value["seed"] = "x".into();
        assert!(IdentityRecord::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}

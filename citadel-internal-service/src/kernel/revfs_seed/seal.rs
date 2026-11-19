//! An account's RE-VFS seed at rest: AES-256-GCM under the agent's vault key (symmetric, so
//! post-quantum safe). Pure: the nonce is the caller's fresh randomness.
//!
//! The associated data names the format and the account, so a sealed seed copied into another
//! account's record does not open there.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use zeroize::Zeroizing;

pub(crate) const SEED_LEN: usize = 32;
pub(crate) const VAULT_KEY_LEN: usize = 32;
pub(crate) const NONCE_LEN: usize = 12;
const VERSION: u8 = 1;
const SEAL_AD: &[u8] = b"citadel-agent-revfs-seed-v1";
const TAG_LEN: usize = 16;
const SEALED_LEN: usize = 1 + NONCE_LEN + SEED_LEN + TAG_LEN;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SeedSealError {
    /// Not a record this version wrote.
    Malformed,
    /// The tag does not verify: another vault key, another account, or an altered record.
    DoesNotOpen,
}

fn associated_data(cid: u64) -> Vec<u8> {
    [SEAL_AD, &[0], &cid.to_be_bytes()].concat()
}

fn cipher(vault_key: &[u8; VAULT_KEY_LEN]) -> Aes256Gcm {
    Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(vault_key))
}

/// `version ‖ nonce ‖ AES-256-GCM(seed)`.
pub(crate) fn seal(
    seed: &[u8; SEED_LEN],
    vault_key: &[u8; VAULT_KEY_LEN],
    cid: u64,
    nonce: [u8; NONCE_LEN],
) -> Vec<u8> {
    let aad = associated_data(cid);
    let ciphertext = cipher(vault_key)
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: seed,
                aad: &aad,
            },
        )
        .unwrap_or_else(|_| unreachable!("a 32-byte message is within AES-GCM's limit"));
    [&[VERSION][..], &nonce, &ciphertext].concat()
}

pub(crate) fn open(
    sealed: &[u8],
    vault_key: &[u8; VAULT_KEY_LEN],
    cid: u64,
) -> Result<Zeroizing<[u8; SEED_LEN]>, SeedSealError> {
    if sealed.len() != SEALED_LEN || sealed[0] != VERSION {
        return Err(SeedSealError::Malformed);
    }
    let (nonce, ciphertext) = sealed[1..].split_at(NONCE_LEN);
    let aad = associated_data(cid);
    let plain = Zeroizing::new(
        cipher(vault_key)
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| SeedSealError::DoesNotOpen)?,
    );
    let mut seed = Zeroizing::new([0u8; SEED_LEN]);
    seed.copy_from_slice(&plain);
    Ok(seed)
}

#[cfg(test)]
mod tests {
    //! Control: drop `cid` from `associated_data` → a seed sealed for one account opens for
    //! another and `the_seal_binds_the_vault_key_and_the_account` goes red.
    use super::*;

    const SEED: [u8; 32] = [7; 32];
    const KEY: [u8; 32] = [1; 32];

    #[test]
    fn the_seal_binds_the_vault_key_and_the_account() {
        let sealed = seal(&SEED, &KEY, 42, [2; 12]);
        assert_eq!(*open(&sealed, &KEY, 42).unwrap(), SEED);
        assert_eq!(open(&sealed, &[9; 32], 42), Err(SeedSealError::DoesNotOpen));
        assert_eq!(open(&sealed, &KEY, 43), Err(SeedSealError::DoesNotOpen));
        let mut tampered = sealed.clone();
        tampered[20] ^= 1;
        assert_eq!(open(&tampered, &KEY, 42), Err(SeedSealError::DoesNotOpen));
    }

    #[test]
    fn the_sealed_record_never_holds_the_seed() {
        let sealed = seal(&SEED, &KEY, 42, [2; 12]);
        assert!(!sealed.windows(SEED_LEN).any(|w| w == SEED));
    }

    #[test]
    fn a_record_of_another_shape_is_malformed() {
        let sealed = seal(&SEED, &KEY, 42, [2; 12]);
        assert_eq!(open(&sealed[1..], &KEY, 42), Err(SeedSealError::Malformed));
        let mut other_version = sealed;
        other_version[0] = 2;
        assert_eq!(
            open(&other_version, &KEY, 42),
            Err(SeedSealError::Malformed)
        );
    }
}

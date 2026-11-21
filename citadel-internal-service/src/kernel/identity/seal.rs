//! The identity seed at rest (identity doc §4.4): sealed with AES-256-GCM under
//! `k_unlock = Argon2id(unlock password, salt)`. Symmetric, so post-quantum safe; a wrong
//! password fails the tag. Pure: the salt and nonce are the caller's fresh randomness.
//!
//! The associated data names the format and the identity's fingerprint, so a sealed seed moved
//! under another identity's record does not open.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub(crate) const SALT_LEN: usize = 32;
pub(crate) const NONCE_LEN: usize = 12;
const SEAL_AD: &[u8] = b"citadel-agent-identity-seal-v1";
const SEED_LEN: usize = citadel_identity::SEED_LEN;

/// Argon2id cost, recorded beside each sealed seed so a later raise still opens older seals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KdfParams {
    pub(crate) mem_kib: u32,
    pub(crate) iterations: u32,
    pub(crate) lanes: u32,
}

/// What a new seal uses: above the SDK's sign-in floor (`KsfParams::FLOOR`, OWASP's 19 MiB, 2
/// passes, 1 lane), since this one key guards every server password the identity derives.
pub(crate) const SEAL_PARAMS: KdfParams = KdfParams {
    mem_kib: 64 * 1024,
    iterations: 3,
    lanes: 1,
};

/// Refused on open: a record whose parameters someone lowered is not trusted to stretch.
pub(crate) const MIN_PARAMS: KdfParams = KdfParams {
    mem_kib: 19 * 1024,
    iterations: 2,
    lanes: 1,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SealedSeed {
    pub(crate) kdf: KdfParams,
    pub(crate) salt: [u8; SALT_LEN],
    pub(crate) nonce: [u8; NONCE_LEN],
    /// The 32-byte seed and the 16-byte tag.
    pub(crate) ciphertext: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SealError {
    WeakParams,
    Kdf(String),
    /// The tag does not verify: the wrong password, or a record that was altered.
    WrongPassword,
    Malformed,
}

fn unlock_key(
    password: &[u8],
    salt: &[u8; SALT_LEN],
    kdf: KdfParams,
) -> Result<Zeroizing<Vec<u8>>, SealError> {
    let config = argon2::Config {
        ad: SEAL_AD,
        hash_length: 32,
        lanes: kdf.lanes,
        mem_cost: kdf.mem_kib,
        secret: &[],
        thread_mode: argon2::ThreadMode::Sequential,
        time_cost: kdf.iterations,
        variant: argon2::Variant::Argon2id,
        version: argon2::Version::Version13,
    };
    argon2::hash_raw(password, salt, &config)
        .map(Zeroizing::new)
        .map_err(|err| SealError::Kdf(err.to_string()))
}

fn meets(kdf: KdfParams, floor: KdfParams) -> bool {
    kdf.mem_kib >= floor.mem_kib && kdf.iterations >= floor.iterations && kdf.lanes >= floor.lanes
}

fn associated_data(id_fp: &str) -> Vec<u8> {
    [SEAL_AD, &[0], id_fp.as_bytes()].concat()
}

pub(crate) fn seal(
    seed: &[u8; SEED_LEN],
    password: &[u8],
    id_fp: &str,
    salt: [u8; SALT_LEN],
    nonce: [u8; NONCE_LEN],
    kdf: KdfParams,
) -> Result<SealedSeed, SealError> {
    if !meets(kdf, MIN_PARAMS) {
        return Err(SealError::WeakParams);
    }
    let key = unlock_key(password, &salt, kdf)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let aad = associated_data(id_fp);
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: seed,
                aad: &aad,
            },
        )
        .map_err(|_| SealError::Malformed)?;
    Ok(SealedSeed {
        kdf,
        salt,
        nonce,
        ciphertext,
    })
}

pub(crate) fn unseal(
    sealed: &SealedSeed,
    password: &[u8],
    id_fp: &str,
) -> Result<Zeroizing<[u8; SEED_LEN]>, SealError> {
    if !meets(sealed.kdf, MIN_PARAMS) {
        return Err(SealError::WeakParams);
    }
    let key = unlock_key(password, &sealed.salt, sealed.kdf)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let aad = associated_data(id_fp);
    let plain = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&sealed.nonce),
                Payload {
                    msg: &sealed.ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| SealError::WrongPassword)?,
    );
    let mut seed = Zeroizing::new([0u8; SEED_LEN]);
    if plain.len() != SEED_LEN {
        return Err(SealError::Malformed);
    }
    seed.copy_from_slice(&plain);
    Ok(seed)
}

#[cfg(test)]
mod tests {
    //! Control: drop `id_fp` from `associated_data` → a seal under one fingerprint opens under
    //! another and `the_seal_binds_the_password_and_the_identity` goes red.
    use super::*;

    const SEED: [u8; 32] = [7; 32];
    // Fast but legal test parameters: the floor itself.
    const TEST_PARAMS: KdfParams = MIN_PARAMS;

    #[test]
    fn the_seal_parameters_meet_the_sdk_sign_in_floor() {
        let floor = citadel_user::auth::pq::record::KsfParams::FLOOR;
        assert!(SEAL_PARAMS.mem_kib >= floor.mem_kib);
        assert!(SEAL_PARAMS.iterations >= floor.iterations);
        assert!(SEAL_PARAMS.lanes >= floor.lanes);
        assert_eq!(
            (MIN_PARAMS.mem_kib, MIN_PARAMS.iterations, MIN_PARAMS.lanes),
            (floor.mem_kib, floor.iterations, floor.lanes),
            "the open floor is the SDK's"
        );
    }

    #[test]
    fn the_seal_binds_the_password_and_the_identity() {
        let sealed = seal(
            &SEED,
            b"correct horse",
            "fp-a",
            [1; 32],
            [2; 12],
            TEST_PARAMS,
        )
        .unwrap();
        assert_eq!(*unseal(&sealed, b"correct horse", "fp-a").unwrap(), SEED);
        assert_eq!(
            unseal(&sealed, b"wrong horse", "fp-a"),
            Err(SealError::WrongPassword)
        );
        assert_eq!(
            unseal(&sealed, b"correct horse", "fp-b"),
            Err(SealError::WrongPassword)
        );
        let mut tampered = sealed.clone();
        tampered.ciphertext[0] ^= 1;
        assert_eq!(
            unseal(&tampered, b"correct horse", "fp-a"),
            Err(SealError::WrongPassword)
        );
    }

    #[test]
    fn lowered_parameters_are_refused_both_ways() {
        let weak = KdfParams {
            mem_kib: 1024,
            ..MIN_PARAMS
        };
        assert_eq!(
            seal(&SEED, b"pw", "fp", [1; 32], [2; 12], weak),
            Err(SealError::WeakParams)
        );
        let mut sealed = seal(&SEED, b"pw", "fp", [1; 32], [2; 12], TEST_PARAMS).unwrap();
        sealed.kdf = weak;
        assert_eq!(unseal(&sealed, b"pw", "fp"), Err(SealError::WeakParams));
    }
}

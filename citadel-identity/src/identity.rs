//! The identity key and the per-server password (identity doc §3.2, §3.3 rev b).
//!
//! Two sub-keys of the seed, each under its own HMAC label, so a leaked password key cannot sign
//! and a leaked signing key cannot sign in:
//!
//! ```text
//! xi       = HMAC-SHA3-256(seed, "citadel-identity-sign-v1")     -> ML-DSA-65 KeyGen(xi)
//! k_pw     = HMAC-SHA3-256(seed, "citadel-identity-pw-v1")
//! pw_bytes = HMAC-SHA3-256(k_pw, "citadel-server-pw-v2" ‖ 0x00 ‖ domain ‖ 0x00 ‖ username)
//! password = base64url_nopad(pw_bytes)                            -> 43 characters
//! ```

use crate::binding::{binding_blob, BindingBlob};
use crate::codec::{base64url_encode, hmac_sha3_256};
use crate::domain::CanonicalDomain;
use crate::keys::{fingerprint, MlDsa65Pk, Signer, SEED_LEN};
use crate::names::NameError;
use std::fmt;
use zeroize::Zeroizing;

pub const SIGN_SUBKEY_LABEL: &[u8] = b"citadel-identity-sign-v1";
pub const PASSWORD_SUBKEY_LABEL: &[u8] = b"citadel-identity-pw-v1";
pub const SERVER_PASSWORD_DOMAIN: &[u8] = b"citadel-server-pw-v2";
/// Characters of a derived password: base64url of 32 bytes, unpadded.
pub const PASSWORD_LEN: usize = 43;

/// A person's identity, expanded from the 32-byte seed the caller holds sealed. The seed itself
/// is not kept; the signing key and the password key are wiped on drop.
pub struct Identity {
    pub(crate) signer: Signer,
    k_pw: Zeroizing<[u8; 32]>,
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Identity({})", self.id_fp())
    }
}

/// A derived per-server password. Never displayed or logged; wiped on drop.
pub struct Password(Zeroizing<String>);

impl Password {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(<redacted>)")
    }
}

impl Identity {
    pub fn from_seed(seed: &[u8; SEED_LEN]) -> Identity {
        let xi = Zeroizing::new(hmac_sha3_256(seed, &[SIGN_SUBKEY_LABEL]));
        let k_pw = Zeroizing::new(hmac_sha3_256(seed, &[PASSWORD_SUBKEY_LABEL]));
        Identity {
            signer: Signer::from_xi(&xi),
            k_pw,
        }
    }

    pub fn pk(&self) -> &[u8; 1952] {
        self.signer.pk().as_bytes()
    }

    pub fn public_key(&self) -> &MlDsa65Pk {
        self.signer.pk()
    }

    /// 16 base32 characters of `SHA3-256("citadel-identity-fp-v1" ‖ 0x00 ‖ pk)[..10]`.
    pub fn id_fp(&self) -> String {
        fingerprint(self.signer.pk())
    }

    /// The password this identity signs in to `username` on `domain` with.
    pub fn derive_password(&self, domain: &CanonicalDomain, username: &str) -> Password {
        let pw_bytes = Zeroizing::new(hmac_sha3_256(
            &self.k_pw[..],
            &[
                SERVER_PASSWORD_DOMAIN,
                &[0],
                domain.as_str().as_bytes(),
                &[0],
                username.as_bytes(),
            ],
        ));
        Password(Zeroizing::new(base64url_encode(&pw_bytes[..])))
    }

    /// This identity's binding to `username` on `domain`, issued at `issued_at_s` (Unix seconds,
    /// the caller's clock). Refuses a name `parse_username` refuses.
    pub fn sign_binding(
        &self,
        username: &str,
        domain: &CanonicalDomain,
        issued_at_s: u64,
    ) -> Result<BindingBlob, NameError> {
        binding_blob(&self.signer, username, domain, issued_at_s)
    }
}

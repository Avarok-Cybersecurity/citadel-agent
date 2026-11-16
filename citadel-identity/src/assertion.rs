//! The sign-in assertion of "Sign in with Citadel" (master plan §9a D-B): after the code and
//! verifier check, the server returns an ML-DSA-65 signed `{identity, device, aud, exp, nonce}`
//! that the relying app (e.g. `sw-mcp`) verifies against the issuer's pinned key.
//!
//! ```text
//! body = u16be(len aud) ‖ aud ‖ u16be(len device) ‖ device ‖ u64be(exp_s) ‖ nonce[32] ‖ identity_pk[1952]
//! sig  = ML-DSA-65.Sign(sk_server, "citadel-sign-in-assertion-v1" ‖ 0x00 ‖ body)
//! blob = 0x01 ‖ body ‖ sig[3309]
//! ```
//!
//! Audience-bound: an assertion issued for one app is refused by every other, and one bound to
//! another nonce (another sign-in attempt) or past its expiry is refused.

use crate::codec::{domain_separated, put_str16, Reader};
use crate::keys::{verify, MlDsa65Pk, SignatureError, PUBLIC_KEY_LEN, SIGNATURE_LEN};
use crate::server::{ServerKey, NONCE_LEN};
use std::fmt;

pub const ASSERTION_DOMAIN: &[u8] = b"citadel-sign-in-assertion-v1";
pub const ASSERTION_VERSION: u8 = 0x01;
pub const MAX_ASSERTION_FIELD_BYTES: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInAssertion {
    /// The signed-in person's identity public key.
    pub identity: MlDsa65Pk,
    pub device: String,
    /// The relying app the assertion is for.
    pub aud: String,
    /// Unix seconds after which it is refused.
    pub exp_s: u64,
    pub nonce: [u8; NONCE_LEN],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssertionError {
    Malformed(String),
    Version(u8),
    Field(&'static str),
    WrongAudience { expected: String, found: String },
    WrongNonce,
    Expired { exp_s: u64, now_s: u64 },
    Signature(SignatureError),
}

impl fmt::Display for AssertionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(why) => write!(f, "the sign-in assertion is malformed: {why}"),
            Self::Version(v) => write!(
                f,
                "sign-in assertion version {v:#04x} is not {ASSERTION_VERSION:#04x}"
            ),
            Self::Field(name) => write!(
                f,
                "the assertion's {name} must be 1 to {MAX_ASSERTION_FIELD_BYTES} bytes"
            ),
            Self::WrongAudience { expected, found } => {
                write!(f, "the assertion is for {found:?}, not {expected:?}")
            }
            Self::WrongNonce => write!(f, "the assertion answers another sign-in"),
            Self::Expired { exp_s, now_s } => {
                write!(f, "the assertion expired at {exp_s} (now {now_s})")
            }
            Self::Signature(why) => write!(f, "the sign-in assertion: {why}"),
        }
    }
}

impl std::error::Error for AssertionError {}

fn body(a: &SignInAssertion) -> Result<Vec<u8>, AssertionError> {
    for (name, value) in [("aud", &a.aud), ("device", &a.device)] {
        if value.is_empty() || value.len() > MAX_ASSERTION_FIELD_BYTES {
            return Err(AssertionError::Field(name));
        }
    }
    let mut body =
        Vec::with_capacity(4 + a.aud.len() + a.device.len() + 8 + NONCE_LEN + PUBLIC_KEY_LEN);
    put_str16(&mut body, &a.aud).map_err(|_| AssertionError::Field("aud"))?;
    put_str16(&mut body, &a.device).map_err(|_| AssertionError::Field("device"))?;
    body.extend_from_slice(&a.exp_s.to_be_bytes());
    body.extend_from_slice(&a.nonce);
    body.extend_from_slice(a.identity.as_bytes());
    Ok(body)
}

/// The bytes the assertion's signature covers.
pub fn assertion_message(a: &SignInAssertion) -> Result<Vec<u8>, AssertionError> {
    Ok(domain_separated(ASSERTION_DOMAIN, &body(a)?))
}

impl ServerKey {
    pub fn sign_assertion(&self, a: &SignInAssertion) -> Result<Vec<u8>, AssertionError> {
        let body = body(a)?;
        let sig = self.signer.sign(&domain_separated(ASSERTION_DOMAIN, &body));
        let mut blob = Vec::with_capacity(1 + body.len() + SIGNATURE_LEN);
        blob.push(ASSERTION_VERSION);
        blob.extend_from_slice(&body);
        blob.extend_from_slice(&sig);
        Ok(blob)
    }
}

/// Accepts `blob` only as `issuer`'s assertion for `expect_aud`, answering `expect_nonce`,
/// unexpired at `now_s` (the caller's clock).
pub fn verify_assertion(
    blob: &[u8],
    issuer: &MlDsa65Pk,
    expect_aud: &str,
    expect_nonce: &[u8; NONCE_LEN],
    now_s: u64,
) -> Result<SignInAssertion, AssertionError> {
    let mut reader = Reader::new(blob);
    let version = reader.u8("version").map_err(AssertionError::Malformed)?;
    if version != ASSERTION_VERSION {
        return Err(AssertionError::Version(version));
    }
    let aud = reader.str16("aud").map_err(AssertionError::Malformed)?;
    let device = reader.str16("device").map_err(AssertionError::Malformed)?;
    let exp_s = reader.u64("exp").map_err(AssertionError::Malformed)?;
    let nonce = reader
        .array::<NONCE_LEN>("nonce")
        .map_err(AssertionError::Malformed)?;
    let identity = reader
        .take(PUBLIC_KEY_LEN, "identity")
        .map_err(AssertionError::Malformed)?;
    let sig = reader
        .take(SIGNATURE_LEN, "signature")
        .map_err(AssertionError::Malformed)?;
    reader.finish().map_err(AssertionError::Malformed)?;

    let assertion = SignInAssertion {
        identity: MlDsa65Pk::from_bytes(identity).map_err(AssertionError::Signature)?,
        device: device.to_string(),
        aud: aud.to_string(),
        exp_s,
        nonce,
    };
    verify(issuer, &assertion_message(&assertion)?, sig).map_err(AssertionError::Signature)?;
    if assertion.aud != expect_aud {
        return Err(AssertionError::WrongAudience {
            expected: expect_aud.to_string(),
            found: assertion.aud,
        });
    }
    if assertion.nonce != *expect_nonce {
        return Err(AssertionError::WrongNonce);
    }
    if now_s >= exp_s {
        return Err(AssertionError::Expired { exp_s, now_s });
    }
    Ok(assertion)
}

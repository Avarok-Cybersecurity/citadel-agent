//! The binding: an identity's signature that it owns a username on a server (identity doc §3.4
//! rev b). Stored as `User.metadata["identity_binding"]`, verified by the server on write and by
//! every reader locally.
//!
//! ```text
//! msg  = "citadel-identity-binding-v2" ‖ 0x00 ‖ u16be(len u) ‖ u ‖ u16be(len d) ‖ d ‖ u64be(issued_at_s)
//! blob = 0x02 ‖ u64be(issued_at_s) ‖ u16be(len d) ‖ d ‖ pk[1952] ‖ sig[3309]     = 5272 + len(d) bytes
//! ```

use crate::codec::{base64url_decode, base64url_encode, domain_separated, put_str16, Reader};
use crate::domain::{canonical_domain, CanonicalDomain};
use crate::keys::{
    fingerprint, verify, MlDsa65Pk, SignatureError, Signer, PUBLIC_KEY_LEN, SIGNATURE_LEN,
};
use crate::names::{parse_username, NameError};
use std::fmt;

pub const BINDING_DOMAIN: &[u8] = b"citadel-identity-binding-v2";
pub const BINDING_VERSION: u8 = 0x02;
/// Bytes of a blob around its domain: version, issued_at, the domain's length, pk and sig.
pub const BINDING_FIXED_LEN: usize = 1 + 8 + 2 + PUBLIC_KEY_LEN + SIGNATURE_LEN;
/// How far past the verifier's clock an `issued_at` may be (identity doc §6).
pub const MAX_BINDING_FUTURE_SKEW_S: u64 = 300;
/// The largest base64url text a server stores under `identity_binding`.
pub const MAX_BINDING_BASE64_LEN: usize = 8192;

/// A signed binding, as bytes; `to_base64url` is the stored form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingBlob(Vec<u8>);

impl BindingBlob {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn to_base64url(&self) -> String {
        base64url_encode(&self.0)
    }
}

/// The blob bytes of a stored binding, refusing text over `MAX_BINDING_BASE64_LEN`.
pub fn binding_from_base64url(text: &str) -> Result<Vec<u8>, BindingError> {
    if text.len() > MAX_BINDING_BASE64_LEN {
        return Err(BindingError::Malformed(format!(
            "{} characters, over the {MAX_BINDING_BASE64_LEN} allowed",
            text.len()
        )));
    }
    base64url_decode(text).map_err(BindingError::Malformed)
}

/// What a verified binding proves: this key's owner holds the username on `domain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub pk: MlDsa65Pk,
    pub id_fp: String,
    pub domain: CanonicalDomain,
    pub issued_at_s: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingError {
    Malformed(String),
    Version(u8),
    Username(NameError),
    /// The binding names a domain the verifier does not accept.
    DomainNotAccepted(String),
    FromTheFuture {
        issued_at_s: u64,
        now_s: u64,
    },
    Signature(SignatureError),
}

impl fmt::Display for BindingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(why) => write!(f, "the identity binding is malformed: {why}"),
            Self::Version(v) => write!(f, "identity binding version {v:#04x} is not {BINDING_VERSION:#04x}"),
            Self::Username(why) => write!(f, "the bound username is refused: {why}"),
            Self::DomainNotAccepted(d) => write!(f, "the binding names {d:?}, not a domain of this server"),
            Self::FromTheFuture { issued_at_s, now_s } => write!(
                f,
                "the binding was issued at {issued_at_s}, more than {MAX_BINDING_FUTURE_SKEW_S}s after {now_s}"
            ),
            Self::Signature(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for BindingError {}

/// The signed message for `username` on `domain` issued at `issued_at_s`.
pub fn binding_message(
    username: &str,
    domain: &str,
    issued_at_s: u64,
) -> Result<Vec<u8>, NameError> {
    let mut body = Vec::with_capacity(4 + username.len() + domain.len() + 8);
    put_str16(&mut body, username).map_err(|len| NameError::TooLong { len })?;
    put_str16(&mut body, domain).map_err(|len| NameError::TooLong { len })?;
    body.extend_from_slice(&issued_at_s.to_be_bytes());
    Ok(domain_separated(BINDING_DOMAIN, &body))
}

pub(crate) fn binding_blob(
    signer: &Signer,
    username: &str,
    domain: &CanonicalDomain,
    issued_at_s: u64,
) -> Result<BindingBlob, NameError> {
    parse_username(username)?;
    let message = binding_message(username, domain.as_str(), issued_at_s)?;
    let signature = signer.sign(&message);
    let mut blob = Vec::with_capacity(BINDING_FIXED_LEN + domain.as_str().len());
    blob.push(BINDING_VERSION);
    blob.extend_from_slice(&issued_at_s.to_be_bytes());
    put_str16(&mut blob, domain.as_str()).map_err(|len| NameError::TooLong { len })?;
    blob.extend_from_slice(signer.pk().as_bytes());
    blob.extend_from_slice(&signature);
    Ok(BindingBlob(blob))
}

/// Accepts `blob` only as a valid binding of `expect_username` on a domain in `accepted`, issued
/// no more than `MAX_BINDING_FUTURE_SKEW_S` after `now_s`. Monotonicity against a stored binding
/// is the server's check (it holds the previous `issued_at`).
pub fn verify_binding(
    blob: &[u8],
    expect_username: &str,
    accepted: &[CanonicalDomain],
    now_s: u64,
) -> Result<Verified, BindingError> {
    let mut reader = Reader::new(blob);
    let version = reader.u8("version").map_err(BindingError::Malformed)?;
    if version != BINDING_VERSION {
        return Err(BindingError::Version(version));
    }
    let issued_at_s = reader.u64("issued_at").map_err(BindingError::Malformed)?;
    let domain_text = reader.str16("domain").map_err(BindingError::Malformed)?;
    let pk_bytes = reader
        .take(PUBLIC_KEY_LEN, "public key")
        .map_err(BindingError::Malformed)?;
    let signature = reader
        .take(SIGNATURE_LEN, "signature")
        .map_err(BindingError::Malformed)?;
    reader.finish().map_err(BindingError::Malformed)?;

    parse_username(expect_username).map_err(BindingError::Username)?;
    let domain = canonical_domain(domain_text)
        .ok()
        .filter(|d| d.as_str() == domain_text)
        .ok_or_else(|| BindingError::Malformed(format!("{domain_text:?} is not canonical")))?;
    if !accepted.contains(&domain) {
        return Err(BindingError::DomainNotAccepted(domain_text.to_string()));
    }
    if issued_at_s > now_s.saturating_add(MAX_BINDING_FUTURE_SKEW_S) {
        return Err(BindingError::FromTheFuture { issued_at_s, now_s });
    }
    let pk = MlDsa65Pk::from_bytes(pk_bytes).map_err(BindingError::Signature)?;
    let message = binding_message(expect_username, domain_text, issued_at_s)
        .map_err(BindingError::Username)?;
    verify(&pk, &message, signature).map_err(BindingError::Signature)?;
    Ok(Verified {
        id_fp: fingerprint(&pk),
        pk,
        domain,
        issued_at_s,
    })
}

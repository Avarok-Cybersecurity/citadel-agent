//! Why a `/.well-known/citadel` document, or a refresh of a pinned one, is refused.

use crate::keys::SignatureError;
use crate::well_known::{SERVER_KEY_ALG, WELL_KNOWN_VERSION};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WellKnownError {
    Json(String),
    Version(u8),
    Alg(String),
    Domain(String),
    /// `citadel.wss` is not a `wss://` URL on a canonical host.
    Endpoint(String),
    /// Fetched from a host that is neither the canonical domain nor a former one.
    NotThisServer {
        fetched_from: String,
    },
    Signature(SignatureError),
    /// Older than the document already pinned.
    Stale {
        pinned_issued_at_s: u64,
        issued_at_s: u64,
    },
    /// A different key than the pinned one: a warning with both fingerprints, never a re-pin.
    KeyChanged {
        pinned_fp: String,
        offered_fp: String,
    },
}

impl fmt::Display for WellKnownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(why) => write!(f, "/.well-known/citadel is not valid: {why}"),
            Self::Version(v) => write!(f, "/.well-known/citadel version {v} is not {WELL_KNOWN_VERSION}"),
            Self::Alg(alg) => write!(f, "server key algorithm {alg:?} is not {SERVER_KEY_ALG}"),
            Self::Domain(why) => write!(f, "/.well-known/citadel names a bad domain: {why}"),
            Self::Endpoint(url) => write!(f, "{url:?} is not a wss:// endpoint"),
            Self::NotThisServer { fetched_from } => {
                write!(f, "the document fetched from {fetched_from} does not claim that name")
            }
            Self::Signature(why) => write!(f, "/.well-known/citadel's signature: {why}"),
            Self::Stale { pinned_issued_at_s, issued_at_s } => write!(
                f,
                "the document was issued at {issued_at_s}, before the pinned one ({pinned_issued_at_s})"
            ),
            Self::KeyChanged { pinned_fp, offered_fp } => write!(
                f,
                "the server key changed: pinned {pinned_fp}, offered {offered_fp}"
            ),
        }
    }
}

impl std::error::Error for WellKnownError {}

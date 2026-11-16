//! `https://<domain>/.well-known/citadel` (master plan §5.2): the server's canonical domain, wss
//! endpoint, ML-DSA-65 key and former names, self-signed:
//!
//! ```text
//! sig = ML-DSA-65.Sign(sk_server, "citadel-well-known-v1" ‖ 0x00 ‖ deterministic_cbor(every field but sig))
//! ```
//!
//! The CBOR mirrors the JSON exactly: the same keys, text as text, integers as unsigned, the key
//! as its base64url text. Unknown fields are refused, so nothing unsigned rides along.

use crate::cbor::Cbor;
use crate::codec::{base64url_decode, base64url_encode, domain_separated};
use crate::domain::{canonical_domain, CanonicalDomain};
use crate::keys::{fingerprint, verify, MlDsa65Pk, SignatureError};
use crate::server::ServerKey;
use serde::{Deserialize, Serialize};
use std::fmt;

pub const WELL_KNOWN_DOMAIN: &[u8] = b"citadel-well-known-v1";
pub const WELL_KNOWN_VERSION: u8 = 1;
pub const SERVER_KEY_ALG: &str = "ML-DSA-65";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WellKnownCitadel {
    pub v: u8,
    pub canonical_domain: CanonicalDomain,
    pub citadel_wss: String,
    pub server_key: MlDsa65Pk,
    pub former_domains: Vec<CanonicalDomain>,
    pub issued_at_s: u64,
}

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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Doc {
    v: u8,
    canonical_domain: String,
    citadel: DocCitadel,
    server_key: DocKey,
    former_domains: Vec<String>,
    issued_at: u64,
    sig: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocCitadel {
    wss: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocKey {
    alg: String,
    pk: String,
}

/// The bytes the document's `sig` covers.
pub fn well_known_message(doc: &WellKnownCitadel) -> Result<Vec<u8>, WellKnownError> {
    let pk = doc.server_key.to_base64url();
    let former = doc
        .former_domains
        .iter()
        .map(|d| Cbor::Text(d.as_str()))
        .collect();
    let cbor = Cbor::Map(vec![
        ("v", Cbor::Unsigned(u64::from(doc.v))),
        (
            "canonical_domain",
            Cbor::Text(doc.canonical_domain.as_str()),
        ),
        (
            "citadel",
            Cbor::Map(vec![("wss", Cbor::Text(&doc.citadel_wss))]),
        ),
        (
            "server_key",
            Cbor::Map(vec![
                ("alg", Cbor::Text(SERVER_KEY_ALG)),
                ("pk", Cbor::Text(&pk)),
            ]),
        ),
        ("former_domains", Cbor::Array(former)),
        ("issued_at", Cbor::Unsigned(doc.issued_at_s)),
    ])
    .encode()
    .map_err(WellKnownError::Json)?;
    Ok(domain_separated(WELL_KNOWN_DOMAIN, &cbor))
}

fn canonical(text: &str) -> Result<CanonicalDomain, WellKnownError> {
    let domain = canonical_domain(text).map_err(|why| WellKnownError::Domain(why.to_string()))?;
    if domain.as_str() != text {
        return Err(WellKnownError::Domain(format!("{text:?} is not canonical")));
    }
    Ok(domain)
}

fn check_fields(doc: &WellKnownCitadel) -> Result<(), WellKnownError> {
    if doc.v != WELL_KNOWN_VERSION {
        return Err(WellKnownError::Version(doc.v));
    }
    let host = doc
        .citadel_wss
        .strip_prefix("wss://")
        .ok_or_else(|| WellKnownError::Endpoint(doc.citadel_wss.clone()))?;
    if canonical_domain(host).is_err() {
        return Err(WellKnownError::Endpoint(doc.citadel_wss.clone()));
    }
    for (at, former) in doc.former_domains.iter().enumerate() {
        if *former == doc.canonical_domain || doc.former_domains[..at].contains(former) {
            return Err(WellKnownError::Domain(format!("{former} is listed twice")));
        }
    }
    Ok(())
}

/// Parses and verifies a fetched document: the self-signature, every field, and that
/// `fetched_from` is the canonical domain or a former one.
pub fn parse_well_known(
    json: &[u8],
    fetched_from: &CanonicalDomain,
) -> Result<WellKnownCitadel, WellKnownError> {
    let raw: Doc = serde_json::from_slice(json).map_err(|e| WellKnownError::Json(e.to_string()))?;
    if raw.server_key.alg != SERVER_KEY_ALG {
        return Err(WellKnownError::Alg(raw.server_key.alg));
    }
    let doc = WellKnownCitadel {
        v: raw.v,
        canonical_domain: canonical(&raw.canonical_domain)?,
        citadel_wss: raw.citadel.wss,
        server_key: MlDsa65Pk::from_base64url(&raw.server_key.pk)
            .map_err(WellKnownError::Signature)?,
        former_domains: raw
            .former_domains
            .iter()
            .map(|d| canonical(d))
            .collect::<Result<_, _>>()?,
        issued_at_s: raw.issued_at,
    };
    check_fields(&doc)?;
    let sig = base64url_decode(&raw.sig)
        .map_err(|why| WellKnownError::Signature(SignatureError::MalformedSignature(why)))?;
    verify(&doc.server_key, &well_known_message(&doc)?, &sig).map_err(WellKnownError::Signature)?;
    if doc.canonical_domain != *fetched_from && !doc.former_domains.contains(fetched_from) {
        return Err(WellKnownError::NotThisServer {
            fetched_from: fetched_from.to_string(),
        });
    }
    Ok(doc)
}

impl ServerKey {
    /// The served JSON for `doc`, signed. Refuses a document naming another key.
    pub fn sign_well_known(&self, doc: &WellKnownCitadel) -> Result<Vec<u8>, WellKnownError> {
        if doc.server_key != *self.public_key() {
            return Err(WellKnownError::KeyChanged {
                pinned_fp: fingerprint(self.public_key()),
                offered_fp: fingerprint(&doc.server_key),
            });
        }
        check_fields(doc)?;
        let sig = self.signer.sign(&well_known_message(doc)?);
        let raw = Doc {
            v: doc.v,
            canonical_domain: doc.canonical_domain.to_string(),
            citadel: DocCitadel {
                wss: doc.citadel_wss.clone(),
            },
            server_key: DocKey {
                alg: SERVER_KEY_ALG.to_string(),
                pk: doc.server_key.to_base64url(),
            },
            former_domains: doc.former_domains.iter().map(|d| d.to_string()).collect(),
            issued_at: doc.issued_at_s,
            sig: base64url_encode(&sig),
        };
        serde_json::to_vec(&raw).map_err(|e| WellKnownError::Json(e.to_string()))
    }
}

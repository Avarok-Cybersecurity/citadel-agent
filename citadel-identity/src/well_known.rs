//! `https://<domain>/.well-known/citadel` (master plan §5.2): the server's canonical domain, wss
//! endpoint, ML-DSA-65 key and former names, self-signed:
//!
//! ```text
//! sig = ML-DSA-65.Sign(sk_server, "citadel-well-known-v1" ‖ 0x00 ‖ deterministic_cbor(every field but sig))
//! ```
//!
//! The CBOR mirrors the JSON exactly: the same keys, text as text, integers as unsigned, the key
//! as its base64url text. Unknown fields are refused, so nothing unsigned rides along.
//!
//! `citadel` is optional: absent means an issuer-only server (work.avarok.net) that runs no node
//! and only signs sign-in assertions. Absent is absent in the JSON and in the CBOR, never `null`
//! and never an empty map.

use crate::cbor::Cbor;
use crate::codec::{base64url_decode, base64url_encode, domain_separated};
use crate::domain::{canonical_domain, CanonicalDomain};
use crate::keys::{fingerprint, verify, MlDsa65Pk, SignatureError};
use crate::server::ServerKey;
pub use crate::well_known_error::WellKnownError;
use serde::{Deserialize, Serialize};

pub const WELL_KNOWN_DOMAIN: &[u8] = b"citadel-well-known-v1";
pub const WELL_KNOWN_VERSION: u8 = 1;
pub const SERVER_KEY_ALG: &str = "ML-DSA-65";
/// `well-known-citadel.schema.json` bounds: `issued_at`, `former_domains` and `citadel.wss`.
pub const MIN_ISSUED_AT_S: u64 = 1_700_000_000;
pub const MAX_ISSUED_AT_S: u64 = 4_102_444_800;
pub const MAX_FORMER_DOMAINS: usize = 16;
pub const MAX_WSS_LEN: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WellKnownCitadel {
    pub v: u8,
    pub canonical_domain: CanonicalDomain,
    /// `None` ⇔ the document has no `citadel` member: an issuer-only server.
    pub citadel_wss: Option<String>,
    pub server_key: MlDsa65Pk,
    pub former_domains: Vec<CanonicalDomain>,
    pub issued_at_s: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Doc {
    v: u8,
    canonical_domain: String,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    citadel: Option<DocCitadel>,
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

/// A present `citadel` must be an object: `"citadel": null` is refused, not read as absent.
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<DocCitadel>, D::Error> {
    DocCitadel::deserialize(d).map(Some)
}

/// The bytes the document's `sig` covers.
pub fn well_known_message(doc: &WellKnownCitadel) -> Result<Vec<u8>, WellKnownError> {
    let pk = doc.server_key.to_base64url();
    let former = doc
        .former_domains
        .iter()
        .map(|d| Cbor::Text(d.as_str()))
        .collect();
    let mut entries = vec![
        ("v", Cbor::Unsigned(u64::from(doc.v))),
        (
            "canonical_domain",
            Cbor::Text(doc.canonical_domain.as_str()),
        ),
    ];
    if let Some(wss) = &doc.citadel_wss {
        entries.push(("citadel", Cbor::Map(vec![("wss", Cbor::Text(wss))])));
    }
    entries.extend([
        (
            "server_key",
            Cbor::Map(vec![
                ("alg", Cbor::Text(SERVER_KEY_ALG)),
                ("pk", Cbor::Text(&pk)),
            ]),
        ),
        ("former_domains", Cbor::Array(former)),
        ("issued_at", Cbor::Unsigned(doc.issued_at_s)),
    ]);
    let cbor = Cbor::Map(entries).encode().map_err(WellKnownError::Json)?;
    Ok(domain_separated(WELL_KNOWN_DOMAIN, &cbor))
}

fn canonical(text: &str) -> Result<CanonicalDomain, WellKnownError> {
    let domain = canonical_domain(text).map_err(|why| WellKnownError::Domain(why.to_string()))?;
    if domain.as_str() != text {
        return Err(WellKnownError::Domain(format!("{text:?} is not canonical")));
    }
    Ok(domain)
}

/// The schema's bounds on the raw values, checked before anything is interpreted.
fn check_shape(issued_at: u64, former: usize, wss: Option<&str>) -> Result<(), WellKnownError> {
    if !(MIN_ISSUED_AT_S..=MAX_ISSUED_AT_S).contains(&issued_at) {
        return Err(WellKnownError::Json(format!(
            "issued_at {issued_at} is outside [{MIN_ISSUED_AT_S}, {MAX_ISSUED_AT_S}]"
        )));
    }
    if former > MAX_FORMER_DOMAINS {
        return Err(WellKnownError::Json(format!(
            "{former} former domains, over the {MAX_FORMER_DOMAINS} allowed"
        )));
    }
    let Some(wss) = wss else {
        return Ok(());
    };
    let charset = wss
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".:/_~-".contains(&b));
    if wss.len() > MAX_WSS_LEN || !charset {
        return Err(WellKnownError::Json(format!(
            "citadel.wss must be at most {MAX_WSS_LEN} characters of [a-z0-9.:/_~-]"
        )));
    }
    Ok(())
}

fn check_fields(doc: &WellKnownCitadel) -> Result<(), WellKnownError> {
    if doc.v != WELL_KNOWN_VERSION {
        return Err(WellKnownError::Version(doc.v));
    }
    if let Some(wss) = &doc.citadel_wss {
        let host = wss
            .strip_prefix("wss://")
            .ok_or_else(|| WellKnownError::Endpoint(wss.clone()))?;
        if canonical_domain(host).is_err() {
            return Err(WellKnownError::Endpoint(wss.clone()));
        }
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
    let wss = raw.citadel.map(|c| c.wss);
    check_shape(raw.issued_at, raw.former_domains.len(), wss.as_deref())?;
    if raw.server_key.alg != SERVER_KEY_ALG {
        return Err(WellKnownError::Alg(raw.server_key.alg));
    }
    let doc = WellKnownCitadel {
        v: raw.v,
        canonical_domain: canonical(&raw.canonical_domain)?,
        citadel_wss: wss,
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
        check_shape(
            doc.issued_at_s,
            doc.former_domains.len(),
            doc.citadel_wss.as_deref(),
        )?;
        check_fields(doc)?;
        let sig = self.signer.sign(&well_known_message(doc)?);
        let raw = Doc {
            v: doc.v,
            canonical_domain: doc.canonical_domain.to_string(),
            citadel: doc.citadel_wss.clone().map(|wss| DocCitadel { wss }),
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

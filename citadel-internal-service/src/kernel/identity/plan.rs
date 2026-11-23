//! What a `RegisterIdentity` registers, decided before anything is dialled: the username, the
//! canonical domain it is pinned under, the address dialled, and the server key or fingerprint
//! pinned. Pure; the fetched document arrives as bytes.

use super::pin::normalise_fingerprint;
use citadel_identity::{
    canonical_domain, fingerprint, make_username, parse_well_known, CanonicalDomain, MlDsa65Pk,
    WellKnownCitadel,
};
use std::fmt;

/// Why a fetched document gives nothing to register on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlanError {
    /// A verified issuer-only document (no `citadel` member): the server runs no node.
    NoCitadelEndpoint(CanonicalDomain),
    /// Unverified, another server's, or a name the handle cannot take.
    Invalid(String),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCitadelEndpoint(domain) => write!(
                f,
                "{domain} runs no Citadel node: it only signs sign-in assertions"
            ),
            Self::Invalid(why) => f.write_str(why),
        }
    }
}

/// The node a verified document names. The one place an issuer-only document is refused, for
/// `RegisterIdentity` and `ServerDiscovery` alike; never a guessed `wss://<domain>/citadel`.
pub(crate) fn citadel_endpoint(doc: &WellKnownCitadel) -> Result<String, PlanError> {
    doc.citadel_wss
        .clone()
        .ok_or_else(|| PlanError::NoCitadelEndpoint(doc.canonical_domain.clone()))
}

/// `document`, verified as fetched from `dialled`, with the node it names.
pub(crate) fn verified(
    dialled: &CanonicalDomain,
    document: &[u8],
) -> Result<(WellKnownCitadel, String), PlanError> {
    let doc =
        parse_well_known(document, dialled).map_err(|err| PlanError::Invalid(err.to_string()))?;
    let wss = citadel_endpoint(&doc)?;
    Ok((doc, wss))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) handle: String,
    pub(crate) username: String,
    pub(crate) domain: CanonicalDomain,
    /// What the agent dials: the document's `citadel.wss`, or the typed address when pinned.
    pub(crate) dial: String,
    pub(crate) server_fp: String,
    pub(crate) server_pk: Option<MlDsa65Pk>,
    pub(crate) issued_at_s: Option<u64>,
}

/// The canonical form of what the person typed.
pub(crate) fn dialled(server_addr: &str) -> Result<CanonicalDomain, String> {
    canonical_domain(server_addr.trim()).map_err(|err| err.to_string())
}

/// From a fetched `/.well-known/citadel`: a dialled former domain registers under the
/// document's canonical one, which is what is pinned.
pub(crate) fn from_well_known(
    handle: &str,
    dialled: &CanonicalDomain,
    document: &[u8],
) -> Result<Plan, PlanError> {
    let (doc, dial) = verified(dialled, document)?;
    let username = make_username(handle, &doc.canonical_domain)
        .map_err(|err| PlanError::Invalid(err.to_string()))?;
    Ok(Plan {
        handle: handle.to_string(),
        username,
        server_fp: fingerprint(&doc.server_key),
        server_pk: Some(doc.server_key),
        issued_at_s: Some(doc.issued_at_s),
        dial,
        domain: doc.canonical_domain,
    })
}

/// How a registration finds its server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Discovery {
    /// From its signed `/.well-known/citadel`.
    WellKnown,
    /// A server without HTTPS, pinned to `server_fp` and dialled at `dial`: for a
    /// `RegisterIdentity`, the address typed; for an app session, its configured pin's.
    Pinned { server_fp: String, dial: String },
}

/// For a server without HTTPS: `dial` is dialled and `dialled`, the canonical domain, is what
/// the username and the challenge name; only the operator's fingerprint is pinned until a
/// challenge proves its key.
pub(crate) fn pinned(
    handle: &str,
    dial: &str,
    dialled: CanonicalDomain,
    typed_fp: &str,
) -> Result<Plan, String> {
    let server_fp = normalise_fingerprint(typed_fp)?;
    let username = make_username(handle, &dialled).map_err(|err| err.to_string())?;
    Ok(Plan {
        handle: handle.to_string(),
        username,
        domain: dialled,
        dial: dial.trim().to_string(),
        server_fp,
        server_pk: None,
        issued_at_s: None,
    })
}

#[cfg(test)]
mod tests {
    //! T5 (rev b): dialling a former domain registers and pins the document's canonical one.
    //!
    //! Control: build the username from `dialled` in `from_well_known` → the former domain is
    //! registered and `a_dialled_alias_registers_under_the_canonical_domain` goes red.
    use super::*;

    const HOSTED: &[u8] = include_bytes!(
        "../../../../citadel-identity/tests/contracts/batch-1116/fixtures/well-known-citadel/valid-hosted-tenant.json"
    );
    const ISSUER_ONLY: &[u8] = include_bytes!(
        "../../../../citadel-identity/tests/contracts/batch-1116/fixtures/well-known-citadel/valid-issuer-only.json"
    );

    /// P14. Control: answer `None` with `wss://<domain>/citadel` in `citadel_endpoint` → the
    /// plan dials the control host and this test goes red.
    #[test]
    fn an_issuer_only_server_is_refused_no_citadel_endpoint() {
        let issuer = dialled("work.avarok.net").unwrap();
        let refused = from_well_known("alice", &issuer, ISSUER_ONLY).unwrap_err();
        assert_eq!(refused, PlanError::NoCitadelEndpoint(issuer.clone()));
        assert_eq!(verified(&issuer, ISSUER_ONLY).unwrap_err(), refused);
        // The document itself is valid: the refusal is not a verification failure.
        assert!(parse_well_known(ISSUER_ONLY, &issuer).is_ok());
    }

    #[test]
    fn a_dialled_alias_registers_under_the_canonical_domain() {
        let alias = dialled("https://Main-Old.work.avarok.net/").unwrap();
        let plan = from_well_known("alice", &alias, HOSTED).unwrap();
        assert_eq!(plan.domain.as_str(), "main.work.avarok.net");
        assert_ne!(
            plan.domain, alias,
            "the pinned canonical differs from the dialled alias"
        );
        assert_eq!(plan.username, "alice@main.work.avarok.net");
        assert_eq!(plan.dial, "wss://main.work.avarok.net/citadel");
        assert_eq!(plan.server_fp, "56natocvznoibcd5");
        assert!(plan.server_pk.is_some());
    }

    #[test]
    fn a_document_another_host_serves_or_a_bad_handle_is_refused() {
        let stranger = dialled("acme.example").unwrap();
        assert!(from_well_known("alice", &stranger, HOSTED).is_err());
        let main = dialled("main.work.avarok.net").unwrap();
        assert!(from_well_known("Alice", &main, HOSTED).is_err());
        assert!(from_well_known("alice", &main, b"{}").is_err());
    }

    #[test]
    fn a_pinned_server_is_dialled_as_typed_and_unverified() {
        let typed = "localhost:12400";
        let plan = pinned(
            "alice",
            typed,
            dialled(typed).unwrap(),
            "56NA-TOCV-ZNOI-BCD5",
        )
        .unwrap();
        assert_eq!(plan.username, "alice@localhost:12400");
        assert_eq!(plan.dial, typed);
        assert_eq!(plan.server_fp, "56natocvznoibcd5");
        assert_eq!(plan.server_pk, None);
        assert!(pinned("alice", typed, dialled(typed).unwrap(), "nope").is_err());
    }
}

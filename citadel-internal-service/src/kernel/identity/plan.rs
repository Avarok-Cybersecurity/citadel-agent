//! What a `RegisterIdentity` registers, decided before anything is dialled: the username, the
//! canonical domain it is pinned under, the address dialled, and the server key or fingerprint
//! pinned. Pure; the fetched document arrives as bytes.

use super::pin::normalise_fingerprint;
use citadel_identity::{
    canonical_domain, fingerprint, make_username, parse_well_known, CanonicalDomain, MlDsa65Pk,
};

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
) -> Result<Plan, String> {
    let doc = parse_well_known(document, dialled).map_err(|err| err.to_string())?;
    let username = make_username(handle, &doc.canonical_domain).map_err(|err| err.to_string())?;
    Ok(Plan {
        handle: handle.to_string(),
        username,
        server_fp: fingerprint(&doc.server_key),
        server_pk: Some(doc.server_key),
        issued_at_s: Some(doc.issued_at_s),
        dial: doc.citadel_wss,
        domain: doc.canonical_domain,
    })
}

/// For a server without HTTPS: the typed address is dialled and its canonical form is the
/// domain; only the operator's fingerprint is pinned until a challenge proves its key.
pub(crate) fn pinned(
    handle: &str,
    server_addr: &str,
    dialled: CanonicalDomain,
    typed_fp: &str,
) -> Result<Plan, String> {
    let server_fp = normalise_fingerprint(typed_fp)?;
    let username = make_username(handle, &dialled).map_err(|err| err.to_string())?;
    Ok(Plan {
        handle: handle.to_string(),
        username,
        domain: dialled,
        dial: server_addr.trim().to_string(),
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

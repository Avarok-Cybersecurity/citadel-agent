//! The canonical domain an account's recovery phrase and RE-VFS key are bound to.
//!
//! One derivation, from the server host the agent records for the account (`server_host.rs`),
//! for every use: registration (`RegisterToHypernode::recovery_domain`), a phrase sign-in, a
//! recovery, a regenerated phrase and the RE-VFS key. A phrase bound to one form of the domain
//! and checked against another would never sign in, so no caller canonicalises on its own.
//!
//! The owner's rule: a server with no canonical domain is refused, never registered or signed in
//! to with none.

use citadel_identity::CanonicalDomain;
use citadel_sdk::prelude::{NodeRemote, Ratchet};

/// The canonical domain of `server`, a host as `server_host::from_typed` records it.
pub(crate) fn of_server(server: &str) -> Result<CanonicalDomain, String> {
    citadel_identity::canonical_domain(server)
        .map_err(|err| format!("the server {server} has no canonical domain: {err}"))
}

/// The canonical domain of the server account `cid` is registered to.
pub(crate) async fn of_account<R: Ratchet>(
    remote: &NodeRemote<R>,
    cid: u64,
) -> Result<CanonicalDomain, String> {
    let server = crate::kernel::account_server::account_server(remote, cid)
        .await?
        .ok_or_else(|| format!("this agent holds no account {cid}"))?;
    citadel_identity::canonical_domain(&server.as_requested())
        .map_err(|err| format!("the server of account {cid} has no canonical domain: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recorded_host_is_its_own_canonical_domain() {
        assert_eq!(of_server("acme.example").unwrap().as_str(), "acme.example");
        assert_eq!(
            of_server("127.0.0.1:25000").unwrap().as_str(),
            "127.0.0.1:25000"
        );
    }

    #[test]
    fn a_host_with_no_canonical_form_is_refused_by_name() {
        let refused = of_server("127.0.0.1:025000").unwrap_err();
        assert!(refused.contains("has no canonical domain"), "{refused}");
    }
}

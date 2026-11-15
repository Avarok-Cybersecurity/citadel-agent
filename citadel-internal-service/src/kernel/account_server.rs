//! Which server a Connect's account is on, checked before anything is dialled.
//!
//! The SDK signs in by username, and finds the account by `username_to_cid(username)`
//! alone (citadel_types `user/mod.rs`): the server is whatever that account was
//! registered to. A sign-in the user meant for one workspace therefore reached
//! another whenever the agent's account of that name was registered there. A
//! Connect now names its server, and the agent dials only when the account with
//! that username is registered to that server.
//!
//! The account's server is the host the agent recorded at registration
//! (`server_host.rs`); for an account registered before it recorded one, the
//! WebSocket endpoint the SDK kept, else the address in its connection info.

use crate::kernel::server_address::{classify, ServerAddress};
use citadel_internal_service_types::{ConnectFailure, FailureReason, InternalServiceResponse};
use citadel_sdk::prelude::{NodeRemote, ProtocolRemoteExt, Ratchet};
use std::net::SocketAddr;
use uuid::Uuid;

/// Where an account is registered, as far as it can be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AccountServer {
    /// A `host[:port]` as `server_host::from_typed` writes it.
    Host(String),
    /// Only a resolved address is known (an account registered by address, before hosts were recorded).
    Addr(SocketAddr),
}

/// Why a Connect is refused before it is dialled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    NoSuchAccount,
    OnAnotherServer,
    InvalidServer(String),
}

/// Whether the requested server (canonical host, and what it resolves to) is the account's.
pub(crate) fn decide(account: &AccountServer, requested: &str, resolved: &[SocketAddr]) -> bool {
    match account {
        AccountServer::Host(host) => host == requested,
        AccountServer::Addr(addr) => requested == addr.to_string() || resolved.contains(addr),
    }
}

/// The requested server in the form `server_host` records, so equal servers compare equal.
pub(crate) fn canonical(requested: &str) -> Result<String, String> {
    crate::kernel::server_host::from_typed(&classify(requested)?)
}

impl AccountServer {
    /// The server as a Connect for this account names it.
    pub(crate) fn as_requested(&self) -> String {
        match self {
            AccountServer::Host(host) => host.clone(),
            AccountServer::Addr(addr) => addr.to_string(),
        }
    }
}

/// The account's server; `None` when the agent holds no account `cid`.
pub(crate) async fn account_server<R: Ratchet>(
    remote: &NodeRemote<R>,
    cid: u64,
) -> Result<Option<AccountServer>, String> {
    if let Some(host) = crate::kernel::server_host::load(remote, cid).await? {
        return Ok(Some(AccountServer::Host(host)));
    }
    if let Some(endpoint) = remote
        .server_endpoint(cid)
        .await
        .map_err(|err| err.into_string())?
    {
        return crate::kernel::server_host::from_typed(&ServerAddress::WebSocket(endpoint))
            .map(|h| Some(AccountServer::Host(h)));
    }
    let cnac = remote
        .account_manager()
        .get_persistence_handler()
        .get_cnac_by_cid(cid)
        .await
        .map_err(|err| err.into_string())?;
    Ok(cnac.map(|cnac| AccountServer::Addr(cnac.get_connect_info().addr)))
}

/// Ok when this agent's account `username` is registered to `requested`.
pub(crate) async fn check<R: Ratchet>(
    remote: &NodeRemote<R>,
    username: &str,
    requested: &str,
) -> Result<(), Refusal> {
    let requested = canonical(requested).map_err(Refusal::InvalidServer)?;
    let cid = remote
        .account_manager()
        .get_persistence_handler()
        .get_cid_by_username(username);
    let registered = remote
        .account_manager()
        .get_username_by_cid(cid)
        .await
        .map_err(|err| Refusal::InvalidServer(err.into_string()))?;
    if registered.as_deref() != Some(username) {
        return Err(Refusal::NoSuchAccount);
    }
    let Some(account) = account_server(remote, cid)
        .await
        .map_err(Refusal::InvalidServer)?
    else {
        return Err(Refusal::NoSuchAccount);
    };
    // Resolved only for an account known by address alone; a recorded host compares as text.
    let resolved: Vec<SocketAddr> = match &account {
        AccountServer::Host(_) => Vec::new(),
        AccountServer::Addr(_) => tokio::net::lookup_host(requested.as_str())
            .await
            .map(Iterator::collect)
            .map_err(|err| {
                Refusal::InvalidServer(format!("could not resolve {requested}: {err}"))
            })?,
    };
    if decide(&account, &requested, &resolved) {
        Ok(())
    } else {
        Err(Refusal::OnAnotherServer)
    }
}

impl Refusal {
    pub(crate) fn into_response(
        self,
        username: &str,
        server: &str,
        request_id: Uuid,
    ) -> InternalServiceResponse {
        let (reason_code, message) = match self {
            Refusal::NoSuchAccount => (Some(FailureReason::NoSuchAccount), format!("This computer has no account named {username}")),
            Refusal::OnAnotherServer => (
                Some(FailureReason::AccountOnAnotherServer),
                format!("{username} is registered to a different server than {server}; nothing was dialled"),
            ),
            Refusal::InvalidServer(err) => (None, format!("Cannot sign in to {server}: {err}")),
        };
        InternalServiceResponse::ConnectFailure(ConnectFailure {
            cid: 0,
            message,
            request_id: Some(request_id),
            reason_code,
            admission_challenge: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recorded_host_matches_only_itself() {
        let account = AccountServer::Host("bench.work.avarok.net".into());
        assert!(decide(&account, "bench.work.avarok.net", &[]));
        assert!(!decide(&account, "acme.work.avarok.net", &[]));
    }

    #[test]
    fn an_address_matches_itself_or_what_the_name_resolves_to() {
        let addr: SocketAddr = "10.0.0.7:12400".parse().unwrap();
        let account = AccountServer::Addr(addr);
        assert!(decide(&account, "10.0.0.7:12400", &[]));
        assert!(decide(&account, "citadel.example.com:12400", &[addr]));
        assert!(!decide(
            &account,
            "citadel.example.com:12400",
            &["10.0.0.8:12400".parse().unwrap()]
        ));
    }

    #[test]
    fn the_requested_server_is_compared_as_recorded() {
        assert_eq!(
            canonical("wss://Bench.work.avarok.net/").unwrap(),
            "bench.work.avarok.net"
        );
        assert_eq!(
            canonical("bench.work.avarok.net").unwrap(),
            "bench.work.avarok.net"
        );
        assert!(canonical("").is_err());
    }
}

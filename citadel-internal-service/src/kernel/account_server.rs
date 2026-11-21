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
//! (`server_host.rs`). An account with none was registered by an earlier agent
//! and is refused by name.

use crate::kernel::server_address::classify;
use citadel_internal_service_types::{ConnectFailure, FailureReason, InternalServiceResponse};
use citadel_sdk::prelude::{NodeRemote, Ratchet};
use uuid::Uuid;

/// Where an account is registered: a `host[:port]` as `server_host::from_typed` writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountServer(pub(crate) String);

/// Why a Connect is refused before it is dialled (and a Register, for `OnAnotherServer`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    NoSuchAccount,
    /// The account with that username is on this server, as a Connect would name it.
    OnAnotherServer(String),
    InvalidServer(String),
}

/// Whether the requested server (canonical host) is the account's.
pub(crate) fn decide(account: &AccountServer, requested: &str) -> bool {
    account.0 == requested
}

/// The requested server in the form `server_host` records, so equal servers compare equal.
pub(crate) fn canonical(requested: &str) -> Result<String, String> {
    crate::kernel::server_host::from_typed(&classify(requested)?)
}

impl AccountServer {
    /// The server as a Connect for this account names it.
    pub(crate) fn as_requested(&self) -> String {
        self.0.clone()
    }
}

/// The account's server; `None` when the agent holds no account `cid`, and an error for an
/// account that recorded none.
pub(crate) async fn account_server<R: Ratchet>(
    remote: &NodeRemote<R>,
    cid: u64,
) -> Result<Option<AccountServer>, String> {
    if let Some(host) = crate::kernel::server_host::load(remote, cid).await? {
        return Ok(Some(AccountServer(host)));
    }
    let held = remote
        .account_manager()
        .get_persistence_handler()
        .get_cnac_by_cid(cid)
        .await
        .map_err(|err| err.into_string())?;
    match held {
        None => Ok(None),
        Some(_) => Err(unrecorded(cid)),
    }
}

/// An account this agent holds with no recorded server.
pub(crate) fn unrecorded(cid: u64) -> String {
    format!(
        "account {cid} recorded no server: it was registered by an earlier agent, which this \
         one does not read. Remove the agent's data directory and register again"
    )
}

/// The server of this agent's account `username`, on whichever server it is; `None` when the
/// agent holds no account of that name.
pub(crate) async fn held<R: Ratchet>(
    remote: &NodeRemote<R>,
    username: &str,
) -> Result<Option<AccountServer>, String> {
    let cid = remote
        .account_manager()
        .get_persistence_handler()
        .get_cid_by_username(username);
    let registered = remote
        .account_manager()
        .get_username_by_cid(cid)
        .await
        .map_err(|err| err.into_string())?;
    if registered.as_deref() != Some(username) {
        return Ok(None);
    }
    account_server(remote, cid).await
}

/// Ok when this agent's account `username` is registered to `requested`.
pub(crate) async fn check<R: Ratchet>(
    remote: &NodeRemote<R>,
    username: &str,
    requested: &str,
) -> Result<(), Refusal> {
    let requested = canonical(requested).map_err(Refusal::InvalidServer)?;
    let Some(account) = held(remote, username)
        .await
        .map_err(Refusal::InvalidServer)?
    else {
        return Err(Refusal::NoSuchAccount);
    };
    if decide(&account, &requested) {
        Ok(())
    } else {
        Err(Refusal::OnAnotherServer(account.as_requested()))
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
            Refusal::NoSuchAccount => (
                Some(FailureReason::NoSuchAccount),
                format!("This computer has no account named {username}"),
            ),
            Refusal::OnAnotherServer(held) => (
                Some(FailureReason::AccountOnAnotherServer),
                format!("{username} is registered to {held}, not {server}; nothing was dialled"),
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
        let account = AccountServer("bench.work.avarok.net".into());
        assert!(decide(&account, "bench.work.avarok.net"));
        assert!(!decide(&account, "acme.work.avarok.net"));
    }

    #[test]
    fn an_account_with_no_recorded_server_is_refused_by_name() {
        let why = unrecorded(7);
        assert!(why.contains("registered by an earlier agent"), "{why}");
        assert!(why.contains("Remove the agent's data directory"), "{why}");
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

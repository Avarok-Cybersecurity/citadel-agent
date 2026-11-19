//! Each account's RE-VFS key (SA-10): objects the account stores on its server, or with a peer,
//! are sealed on this device under a key derived from a root the server never holds.
//!
//! The root is the account's `seed_rec`, which this agent draws and seals (D2) until account
//! recovery shows it as the recovery phrase. A Connect establishes the account's key before it dials (`session_key`), and the node reads it from the session map
//! (`key_source`), so a key lives exactly as long as its session.

mod account;
mod seal;
mod vault;

pub(crate) use vault::Vault;
pub use vault::{vault_for, FsRevfsVault, MemoryRevfsVault, RevfsVaultStore};

use crate::kernel::Connection;
use citadel_sdk::prelude::{NodeRemote, Ratchet, RevfsKey, RevfsKeySource};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// The key of this agent's account `username`, on the server it is registered to. The CID is
/// the account's permanent one (`username_to_cid`), which the sign-in keeps.
pub(crate) async fn session_key<R: Ratchet>(
    vault: Option<&Vault>,
    remote: &NodeRemote<R>,
    username: &str,
) -> Result<RevfsKey, String> {
    let cid = remote
        .account_manager()
        .get_persistence_handler()
        .get_cid_by_username(username);
    let vault = vault.ok_or_else(|| {
        "this agent has no RE-VFS vault (CitadelWorkspaceService::revfs_key_source)".to_string()
    })?;
    let server = crate::kernel::account_server::account_server(remote, cid)
        .await?
        .ok_or_else(|| format!("this agent holds no account {cid}"))?;
    let domain = citadel_identity::canonical_domain(&server.as_requested())
        .map_err(|err| format!("the server of account {cid} has no canonical domain: {err}"))?;
    let vault_key = vault.key().await?;
    account::establish(remote, &vault_key, cid, &domain.to_string()).await
}

/// The node's key source: the key of each recorded session, none for any other account.
pub(crate) fn key_source<R: Ratchet>(
    sessions: Arc<RwLock<HashMap<u64, Connection<R>>>>,
) -> impl RevfsKeySource + 'static {
    move |cid: u64| {
        sessions
            .read()
            .get(&cid)
            .map(|session| session.revfs_key.clone())
    }
}

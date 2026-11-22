//! Each account's RE-VFS key (SA-10): objects the account stores on its server, or with a peer,
//! are sealed on this device under a key derived from a root the server never holds.
//!
//! The root is the seed the account's recovery phrase encodes, so the phrase recovers the files
//! too. It is stored sealed when a registration issues the phrase or the phrase recovers the
//! account here (account.rs); an account with no stored root has no key. A Connect establishes
//! the account's key before it dials (`session_key`), and the node reads it from the session map
//! (`key_source`), so a key lives exactly as long as its session. Regenerating the phrase
//! retires the old root and re-keys every indexed object (index.rs, rekey.rs, node.rs).

mod account;
mod dialing;
pub(crate) mod hooks;
pub(crate) mod index;
mod node;
mod node_ports;
mod rekey;
mod roots;
mod seal;
mod usage;
mod vault;

pub(crate) use account::{Adopt, NO_ROOT};
pub(crate) use dialing::Dialing;
pub(crate) use node::resume;
pub(crate) use usage::UseToken;
/// Why a store, pull or delete is refused while the account is re-keyed: try again after.
pub use usage::REKEYING;

/// Why a pull of an object still sealed under a replaced phrase's root is refused for now (a
/// re-key is started): try again after.
pub const AWAITS_REKEY: &str = "This file is still sealed under the account's previous \
     recovery phrase and is being sealed again under the new one; try again shortly";
pub(crate) use vault::Vault;
pub use vault::{vault_for, FsRevfsVault, MemoryRevfsVault, RevfsVaultStore};

use crate::kernel::Connection;
use citadel_sdk::prelude::{NodeRemote, Ratchet, RevfsKey, RevfsKeySource};
use index::{IndexEntry, Location};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

const NO_VAULT: &str = "this agent has no RE-VFS vault (CitadelWorkspaceService::revfs_key_source)";

fn vault_of(vault: Option<&Vault>) -> Result<&Vault, String> {
    vault.ok_or_else(|| NO_VAULT.to_string())
}

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
    let vault = vault_of(vault)?;
    let domain = crate::kernel::recovery_domain::of_account(remote, cid).await?;
    let vault_key = vault.key().await?;
    account::establish(remote, &vault_key, cid, &domain.to_string()).await
}

/// The seed a phrase's words encode, as the SDK issued them (at registration, or regenerated).
pub(crate) fn seed_of_words(words: &[String]) -> Result<zeroize::Zeroizing<[u8; 32]>, String> {
    let typed = zeroize::Zeroizing::new(words.join(" "));
    citadel_sdk::prelude::RecoverySeed::parse(&typed)
        .map(|seed| zeroize::Zeroizing::new(*seed.as_bytes()))
        .map_err(|err| format!("the phrase does not read back: {}", err.into_string()))
}

/// The key the phrase's seed gives `username`'s account (its server's domain is bound in the
/// phrase), for a phrase sign-in on a device that holds no root for the account yet.
pub(crate) fn phrase_key<R: Ratchet>(
    remote: &NodeRemote<R>,
    username: &str,
    phrase: &citadel_sdk::prelude::RecoveryPhrase,
) -> Result<RevfsKey, String> {
    let cid = remote
        .account_manager()
        .get_persistence_handler()
        .get_cid_by_username(username);
    account::key_of(phrase.seed().as_bytes(), phrase.canonical_domain(), cid)
}

/// Takes the seed of the phrase that was just issued for, or typed to recover, account `cid`
/// as its root (`how`). Before anything can use a key: never sent anywhere, never logged.
pub(crate) async fn adopt_phrase<R: Ratchet>(
    vault: Option<&Vault>,
    remote: &NodeRemote<R>,
    cid: u64,
    seed: &[u8; 32],
    how: Adopt,
) -> Result<(), String> {
    let vault = vault_of(vault)?;
    let vault_key = vault.key().await?;
    let _records = vault.accounts.records.lock().await;
    account::adopt(remote, &vault_key, cid, seed, how).await
}

/// A regenerated phrase's seed becomes the account's root; the one it replaces is retired, and
/// kept until every object sealed under it is re-keyed (`resume`). The new root's key, for the
/// session to store under from now on.
pub(crate) async fn rotate<R: Ratchet>(
    vault: Option<&Vault>,
    remote: &NodeRemote<R>,
    cid: u64,
    seed: &[u8; 32],
) -> Result<RevfsKey, String> {
    let vault = vault_of(vault)?;
    let vault_key = vault.key().await?;
    let domain = crate::kernel::recovery_domain::of_account(remote, cid).await?;
    let _records = vault.accounts.records.lock().await;
    let mut held = account::load(remote, &vault_key, cid)
        .await?
        .ok_or_else(|| NO_ROOT.to_string())?;
    held.rotate(seed);
    account::save(remote, &vault_key, cid, &held).await?;
    account::key_of(seed, &domain.to_string(), cid)
}

/// A store of `cid`'s completed: `entry` names it in the account's index.
pub(crate) async fn record_store<R: Ratchet>(
    vault: Option<&Vault>,
    remote: &NodeRemote<R>,
    cid: u64,
    entry: IndexEntry,
) -> Result<(), String> {
    let vault = vault_of(vault)?;
    let _records = vault.accounts.records.lock().await;
    index::update(remote, cid, |index| index.upsert(entry)).await
}

/// A delete (or a take) of `cid`'s succeeded: the object is no longer in its index.
pub(crate) async fn forget<R: Ratchet>(
    vault: Option<&Vault>,
    remote: &NodeRemote<R>,
    cid: u64,
    location: Location,
    path: &str,
) -> Result<(), String> {
    let vault = vault_of(vault)?;
    let _records = vault.accounts.records.lock().await;
    index::update(remote, cid, |index| index.remove(location, path))
        .await
        .map(|_removed| ())
}

/// Whether the object at `location`/`path` still waits to be re-keyed: it is sealed under a
/// retired root, which the session's key does not open.
pub(crate) async fn awaits_rekey<R: Ratchet>(
    remote: &NodeRemote<R>,
    cid: u64,
    location: Location,
    path: &str,
    current: &RevfsKey,
) -> Result<bool, String> {
    let index = index::load(remote, cid).await?;
    Ok(index
        .get(location, path)
        .is_some_and(|entry| entry.key_id != current.id() || entry.stage != index::Stage::Settled))
}

/// A store, pull or delete of `cid` begins: refused while the account is re-keyed. No vault, no
/// RE-VFS key: the SDK refuses the request itself.
pub(crate) fn begin_use(vault: Option<&Vault>, cid: u64) -> Result<Option<UseToken>, String> {
    vault.map(|vault| vault.accounts.begin_use(cid)).transpose()
}

/// The node's key source: the key of each recorded session, or of a session being dialled
/// (`dialing`), none for any other account.
pub(crate) fn key_source<R: Ratchet>(
    sessions: Arc<RwLock<HashMap<u64, Connection<R>>>>,
    dialing: Dialing,
) -> impl RevfsKeySource + 'static {
    move |cid: u64| {
        let recorded = sessions
            .read()
            .get(&cid)
            .map(|session| session.revfs_key.clone());
        recorded.or_else(|| dialing.key(cid))
    }
}

#[cfg(test)]
#[path = "key_source_tests.rs"]
mod key_source_tests;

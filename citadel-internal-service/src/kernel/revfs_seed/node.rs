//! A re-key pass on the node (rekey.rs is the pass itself). It runs in a full session only (the
//! server drops a recovery console's RE-VFS traffic), alone (usage.rs), and is started at every
//! full sign-in, after a regeneration, when a peer connects, and when a pull meets an object
//! that still waits. It does nothing when no object waits.
//!
//! The node reads an account's key from its session (`key_source`), so each step points the
//! session's key at the key that step uses (node_ports.rs), and the pass restores the current
//! key when it ends. A retired root is dropped only once no index entry names its key, under the lock every index
//! update takes, so an entry recorded meanwhile is never orphaned.

use super::index::{self, IndexEntry};
use super::node_ports::NodePorts;
use super::rekey::{self, Keys};
use super::{account, Vault};
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{NodeRemote, Ratchet, RevfsKey, SessionScope};

/// Starts a pass for `cid` in the background, unless one cannot run now.
pub(crate) fn resume<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
) {
    let this = this.clone();
    drop(crate::telemetry::span::spawn_in_current_span(async move {
        if let Err(err) = pass(&this, cid).await {
            warn!(target: "citadel", "[RE-VFS] The re-key of account {cid} did not run: {err}");
        }
    }));
}

async fn pass<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
) -> Result<(), String> {
    let Some(vault) = this.revfs_vault.get() else {
        return Ok(());
    };
    let full = this
        .server_connection_map
        .read()
        .get(&cid)
        .map(|conn| conn.sign_in.scope == SessionScope::Full);
    if full != Some(true) {
        return Ok(());
    }
    let remote = this.remote();
    let domain = crate::kernel::recovery_domain::of_account(remote, cid)
        .await?
        .to_string();
    let vault_key = vault.key().await?;
    // Looked at first without holding the account: a pass with nothing to do never refuses a
    // window's request. Read again once held, as a regeneration may have come between.
    let (keys, waiting) = load_waiting(remote, &vault_key, cid, &domain).await?;
    if waiting.is_empty() && keys.retired.is_empty() {
        return Ok(());
    }
    let Some(_alone) = vault.accounts.begin_rekey(cid) else {
        return Ok(());
    };
    let (keys, waiting) = load_waiting(remote, &vault_key, cid, &domain).await?;
    let ports = NodePorts { this, vault, cid };
    if !waiting.is_empty() {
        let report = rekey::run(&ports, &keys, waiting).await;
        let outcome = match report.finished() {
            true => "every waiting object is re-keyed",
            false => "the old root is kept for the rest",
        };
        info!(target: "citadel", "[RE-VFS] Re-keyed {} object(s) of account {cid}; {} failed; unreachable: {:?}; {outcome}", report.rekeyed, report.failed.len(), report.unreachable);
        for (location, _path, why) in &report.failed {
            warn!(target: "citadel", "[RE-VFS] An object of account {cid} at {location:?} was not re-keyed: {why}");
        }
    }
    // The current root as stored now: a regeneration during the pass may have replaced it.
    let current = retire_unreferenced(vault, remote, &vault_key, cid, &domain).await?;
    ports.point_at(&current)
}

/// The account's keys, and the index entries not settled under the current one.
async fn load_waiting<R: Ratchet>(
    remote: &NodeRemote<R>,
    vault_key: &[u8; 32],
    cid: u64,
    domain: &str,
) -> Result<(Keys, Vec<IndexEntry>), String> {
    let roots = account::load(remote, vault_key, cid)
        .await?
        .ok_or_else(|| super::NO_ROOT.to_string())?;
    let keys = Keys {
        current: account::key_of(&roots.current, domain, cid)?,
        retired: roots
            .retired
            .iter()
            .map(|root| account::key_of(root, domain, cid))
            .collect::<Result<_, _>>()?,
    };
    let waiting = rekey::pending(&index::load(remote, cid).await?, keys.current.id());
    Ok((keys, waiting))
}

/// Drops each retired root no index entry names any longer; the current root's key.
async fn retire_unreferenced<R: Ratchet>(
    vault: &Vault,
    remote: &NodeRemote<R>,
    vault_key: &[u8; 32],
    cid: u64,
    domain: &str,
) -> Result<RevfsKey, String> {
    let _records = vault.accounts.records.lock().await;
    let mut roots = account::load(remote, vault_key, cid)
        .await?
        .ok_or_else(|| super::NO_ROOT.to_string())?;
    let referenced = index::load(remote, cid).await?.referenced();
    if roots.retain_referenced(&referenced, |root| account::key_of(root, domain, cid))? {
        account::save(remote, vault_key, cid, &roots).await?;
        info!(target: "citadel", "[RE-VFS] Account {cid} no longer needs a retired root");
    }
    account::key_of(&roots.current, domain, cid)
}

//! The node's side of a re-key pass (rekey.rs `RekeyPorts`): each step reaches the account's
//! server through its session, or a peer through the SDK's target-locked calls, after pointing
//! the session's key at the key the step uses (the node reads the key from the session).

use super::index::{self, IndexEntry, Location};
use super::rekey::RekeyPorts;
use super::Vault;
use crate::kernel::CitadelWorkspaceService;
use async_trait::async_trait;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::{
    NodeRemote, ProtocolRemoteTargetExt, Ratchet, RevfsKey, SecurityLevel, SessionSecuritySettings,
    TargetLockedRemote, VirtualTargetType,
};
use std::path::{Path, PathBuf};

pub(super) struct NodePorts<'a, T, R: Ratchet> {
    pub this: &'a CitadelWorkspaceService<T, R>,
    pub vault: &'a Vault,
    pub cid: u64,
}

/// A peer's half of the account's RE-VFS, as the SDK's target-locked calls want it.
struct PeerTarget<R: Ratchet> {
    user: VirtualTargetType,
    remote: NodeRemote<R>,
    settings: SessionSecuritySettings,
}

impl<R: Ratchet> TargetLockedRemote<R> for PeerTarget<R> {
    fn user(&self) -> &VirtualTargetType {
        &self.user
    }
    fn remote(&self) -> &NodeRemote<R> {
        &self.remote
    }
    fn target_username(&self) -> Option<&str> {
        None
    }
    fn user_mut(&mut self) -> &mut VirtualTargetType {
        &mut self.user
    }
    fn session_security_settings(&self) -> Option<&SessionSecuritySettings> {
        Some(&self.settings)
    }
}

enum Target<R: Ratchet> {
    Server(citadel_sdk::prelude::ClientServerRemote<R>),
    Peer(PeerTarget<R>),
}

impl<T: IOInterface + Sync, R: Ratchet> NodePorts<'_, T, R> {
    /// Points the session's key at `key`, which the SDK reads for this account's next transfer.
    pub(super) fn point_at(&self, key: &RevfsKey) -> Result<(), String> {
        let mut map = self.this.server_connection_map.write();
        let conn = map.get_mut(&self.cid).ok_or("the session ended")?;
        conn.revfs_key = key.clone();
        Ok(())
    }

    fn target(&self, location: Location, key: &RevfsKey) -> Result<Target<R>, String> {
        self.point_at(key)?;
        let map = self.this.server_connection_map.read();
        let conn = map.get(&self.cid).ok_or("the session ended")?;
        Ok(match location {
            Location::Server => Target::Server(conn.client_server_remote.clone()),
            Location::Peer(peer_cid) => {
                if !conn.peers.contains_key(&peer_cid) {
                    return Err(format!("peer {peer_cid} is not connected"));
                }
                let settings = conn
                    .client_server_remote
                    .session_security_settings()
                    .cloned()
                    .ok_or("the session has no security settings")?;
                Target::Peer(PeerTarget {
                    user: VirtualTargetType::LocalGroupPeer {
                        session_cid: self.cid,
                        peer_cid,
                    },
                    remote: self.this.remote().clone(),
                    settings,
                })
            }
        })
    }
}

macro_rules! on_target {
    ($target:expr, $remote:ident => $call:expr) => {
        match $target {
            Target::Server($remote) => $call,
            Target::Peer($remote) => $call,
        }
    };
}

#[async_trait]
impl<T: IOInterface + Sync, R: Ratchet> RekeyPorts for NodePorts<'_, T, R> {
    fn reachable(&self, location: Location) -> bool {
        let map = self.this.server_connection_map.read();
        match (map.get(&self.cid), location) {
            (None, _) => false,
            (Some(_), Location::Server) => true,
            (Some(conn), Location::Peer(peer_cid)) => conn.peers.contains_key(&peer_cid),
        }
    }

    async fn pull(
        &self,
        location: Location,
        path: &str,
        key: &RevfsKey,
    ) -> Result<PathBuf, String> {
        let target = self.target(location, key)?;
        on_target!(target, remote => remote
            .remote_encrypted_virtual_filesystem_pull(path, Default::default(), false)
            .await)
        .map_err(|err| err.into_string())
    }

    async fn push(
        &self,
        location: Location,
        path: &str,
        level: u8,
        local: &Path,
        key: &RevfsKey,
    ) -> Result<(), String> {
        let level = SecurityLevel::for_value(level as usize)
            .ok_or_else(|| format!("{level} is not a security level"))?;
        let target = self.target(location, key)?;
        on_target!(target, remote => remote
            .remote_encrypted_virtual_filesystem_push(local.to_path_buf(), path, level)
            .await)
        .map_err(|err| err.into_string())
    }

    async fn delete(&self, location: Location, path: &str) -> Result<(), String> {
        let current = self
            .this
            .server_connection_map
            .read()
            .get(&self.cid)
            .map(|conn| conn.revfs_key.clone())
            .ok_or("the session ended")?;
        let target = self.target(location, &current)?;
        on_target!(target, remote => remote
            .remote_encrypted_virtual_filesystem_delete(path)
            .await)
        .map_err(|err| err.into_string())
    }

    async fn discard(&self, local: &Path) {
        if let Err(err) = tokio::fs::remove_file(local).await {
            warn!(target: "citadel", "[RE-VFS] A re-keyed object's local copy was not removed: {err}");
        }
    }

    async fn record(&self, entry: &IndexEntry) -> Result<(), String> {
        let _records = self.vault.accounts.records.lock().await;
        index::update(self.this.remote(), self.cid, |index| {
            index.upsert(entry.clone())
        })
        .await
    }
}

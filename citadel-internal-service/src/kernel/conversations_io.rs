//! The agent's side of the conversation store's seams (kernel/conversations).

use crate::kernel::conversations::kv::{ConversationKv, HostKv, KvResult};
use crate::kernel::conversations::ConversationIo;
use crate::kernel::ilm::HostIo;
use crate::kernel::session_route::SessionRoute;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_connector::messenger::CompressionHint;
use citadel_internal_service_types::{InternalServiceResponse, SecurityLevel};
use citadel_sdk::prelude::Ratchet;
use futures::future::BoxFuture;
use std::sync::Arc;
use uuid::Uuid;

impl<T: IOInterface + Sync, R: Ratchet> ConversationKv for CitadelWorkspaceService<T, R> {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, KvResult<Option<Vec<u8>>>> {
        Box::pin(async move { HostKv(self.host_io()).get(key).await })
    }
    fn set<'a>(&'a self, key: &'a str, value: Vec<u8>) -> BoxFuture<'a, KvResult<()>> {
        Box::pin(async move { HostKv(self.host_io()).set(key, value).await })
    }
    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, KvResult<()>> {
        Box::pin(async move { HostKv(self.host_io()).delete(key).await })
    }
    fn keys(&self) -> BoxFuture<'_, KvResult<Vec<String>>> {
        Box::pin(async move { HostKv(self.host_io()).keys().await })
    }
}

impl<T: IOInterface + Sync, R: Ratchet> CitadelWorkspaceService<T, R> {
    pub(crate) fn host_io(&self) -> Arc<dyn HostIo> {
        Arc::new(self.clone())
    }
}

impl<T: IOInterface + Sync, R: Ratchet> ConversationIo for CitadelWorkspaceService<T, R> {
    fn kv(&self) -> &dyn ConversationKv {
        self
    }

    fn publish(&self, cid: u64, response: InternalServiceResponse) -> usize {
        crate::kernel::membership::subscribers_of(self, cid)
            .map(|subs| {
                SessionRoute::new(subs, self.tx_to_localhost_clients.clone())
                    .send(response)
                    .len()
            })
            .unwrap_or(0)
    }

    fn send_p2p(
        &self,
        cid: u64,
        peer: u64,
        bytes: Vec<u8>,
    ) -> BoxFuture<'static, Result<(), String>> {
        let host = self.ilm_hosts.get(cid);
        Box::pin(async move {
            let host = host.ok_or_else(|| format!("session {cid} has no agent-hosted ILM"))?;
            // Every P2P command the UI sends is a CBOR command (compression-hints.ts).
            host.send(
                peer,
                Uuid::new_v4(),
                bytes,
                SecurityLevel::Standard,
                Some(CompressionHint::CborCommand),
            )
            .await
        })
    }

    fn now_ms(&self) -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_millis() as f64)
    }

    fn new_id(&self) -> String {
        Uuid::new_v4().to_string()
    }

    fn account_username(&self, cid: u64) -> String {
        self.server_connection_map
            .read()
            .get(&cid)
            .map(|conn| conn.username.clone())
            .unwrap_or_default()
    }

    fn peer_username(&self, cid: u64, peer: u64) -> Option<String> {
        self.peer_username_cache.read().get(&(cid, peer)).cloned()
    }

    fn knows_peer(&self, cid: u64, peer: u64) -> BoxFuture<'static, bool> {
        let connected = self
            .server_connection_map
            .read()
            .get(&cid)
            .is_some_and(|conn| conn.peers.contains_key(&peer));
        let remote = self.remote().clone();
        Box::pin(async move {
            if connected {
                return true;
            }
            match remote.account_manager().get_hyperlan_peer_list(cid).await {
                Ok(peers) => peers.is_some_and(|peers| peers.contains(&peer)),
                // Unanswerable is not "a stranger": the UI's gate shows the
                // message when the registry cannot be read, and so does this.
                Err(err) => {
                    citadel_sdk::logging::warn!(target: "citadel", "[CONVERSATIONS] {cid}: peer list unreadable ({err:?}); treating {peer} as known");
                    true
                }
            }
        })
    }
    fn raise_notice(&self, cid: u64, source: crate::kernel::notices::decide::NoticeSource) {
        CitadelWorkspaceService::raise_notice(self, cid, source)
    }
    fn rows_changed(&self) {
        CitadelWorkspaceService::rows_changed(self)
    }
}

//! The agent-side store under an agent-hosted ILM: the agent's own LocalDB.
//!
//! The browser's ILM reaches this same storage through `LocalDBGetKV` /
//! `LocalDBSetKV` requests, which `requests/local_db/get_kv.rs` and `set_kv.rs`
//! serve as `generate_remote(cid, peer_cid).get/set(key)` -- and the browser
//! always sends `peer_cid: None`. [`LocalDbAccess for NodeRemote`] makes the
//! identical call, so a key written by either host is the same (cid, peer 0,
//! key) entry in the account's byte map, and an agent-hosted ILM picks up the
//! queues and tracker state a browser's ILM left behind.
//!
//! Keys arrive fully formed from `CitadelWorkspaceBackend::storage_key` and are
//! stored verbatim; nothing here may prefix or re-encode them.
use crate::kernel::requests::local_db::generate_remote;
use citadel_internal_service_connector::messenger::ilm::BackendError;
use citadel_internal_service_connector::messenger::kv_store::{IlmKvStore, KvResult};
use citadel_internal_service_connector::messenger::WrappedMessage;
use citadel_sdk::backend_kv_store::BackendHandler;
use citadel_sdk::prelude::{async_trait, NodeRemote, Ratchet};
use uuid::Uuid;

/// The agent's per-account key/value storage, as the LocalDB handlers use it.
///
/// A trait so the ILM host can be exercised without a running node; the only
/// production implementation is the one for `NodeRemote` below.
#[async_trait]
pub trait LocalDbAccess: Send + Sync + 'static {
    async fn get(&self, cid: u64, key: &str) -> Result<Option<Vec<u8>>, String>;
    async fn set(&self, cid: u64, key: &str, value: Vec<u8>) -> Result<(), String>;
}

/// The browser's requests carry `peer_cid: None`; so does every call here.
const BROWSER_ILM_PEER_CID: Option<u64> = None;

#[async_trait]
impl<R: Ratchet> LocalDbAccess for NodeRemote<R> {
    async fn get(&self, cid: u64, key: &str) -> Result<Option<Vec<u8>>, String> {
        let remote = generate_remote(self, cid, BROWSER_ILM_PEER_CID)
            .await
            .map_err(|err| err.into_string())?;
        remote.get(key).await.map_err(|err| err.into_string())
    }

    async fn set(&self, cid: u64, key: &str, value: Vec<u8>) -> Result<(), String> {
        let remote = generate_remote(self, cid, BROWSER_ILM_PEER_CID)
            .await
            .map_err(|err| err.into_string())?;
        remote
            .set(key, value)
            .await
            .map(|_previous| ())
            .map_err(|err| err.into_string())
    }
}

/// An [`IlmKvStore`] over one account's LocalDB.
pub struct AgentKvStore<D: LocalDbAccess> {
    cid: u64,
    db: D,
}

impl<D: LocalDbAccess> AgentKvStore<D> {
    pub fn new(cid: u64, db: D) -> Self {
        Self { cid, db }
    }
}

fn read_failed(key: &str, reason: String) -> BackendError<WrappedMessage> {
    BackendError::StorageError(format!("Failed to read key={key}: {reason}"))
}

fn write_failed(key: &str, reason: String) -> BackendError<WrappedMessage> {
    BackendError::StorageError(format!("Writing key={key} failed: {reason}"))
}

#[async_trait]
impl<D: LocalDbAccess> IlmKvStore for AgentKvStore<D> {
    async fn get(&self, key: &str) -> KvResult<Option<Vec<u8>>> {
        self.db
            .get(self.cid, key)
            .await
            .map_err(|reason| read_failed(key, reason))
    }

    /// No request travels, so there is nothing for `request_id` to name.
    async fn set(&self, _request_id: Uuid, key: &str, value: Vec<u8>) -> KvResult<()> {
        self.db
            .set(self.cid, key, value)
            .await
            .map_err(|reason| write_failed(key, reason))
    }

    /// Sequential: the batch exists to save WebSocket round trips, and there
    /// are none here. Stops at the first failure -- a partial read is an error.
    async fn get_many(&self, keys: &[String]) -> KvResult<Vec<Option<Vec<u8>>>> {
        let mut values = Vec::with_capacity(keys.len());
        for key in keys {
            values.push(self.get(key).await?);
        }
        Ok(values)
    }

    async fn set_many(&self, entries: &[(String, Vec<u8>)]) -> KvResult<()> {
        for (key, value) in entries {
            self.set(Uuid::nil(), key, value.clone()).await?;
        }
        Ok(())
    }
}

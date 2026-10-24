//! A member's own grants, on this agent, to every app it runs for them.
//!
//! Stored in the account's own LocalDB bucket under `agent:` keys, which no window may read,
//! write, delete, list or clear (requests/local_db): only the owned `AppsGetAgentSettings` and
//! `AppsSetAgentSettings` reach them. An unset grant is not a grant.

use crate::kernel::conversations::kv::{ConversationKv, HostKv};
use crate::kernel::ilm::HostIo;
use futures::future::BoxFuture;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// The LocalDB keys only the agent itself reads and writes.
pub(crate) const AGENT_KEY_PREFIX: &str = "agent:";
/// "Let apps reach my private network from this computer": `[1]` on, `[0]` off.
pub(crate) const PRIVATE_NETWORK_KEY: &str = "agent:apps_private_network";

/// What host egress reads of a member's grants.
pub(crate) trait MemberGrants: Send + Sync {
    fn private_network(&self, cid: u64) -> BoxFuture<'_, Result<Option<bool>, String>>;
}

fn decode(stored: Option<Vec<u8>>) -> Result<Option<bool>, String> {
    match stored.as_deref() {
        None => Ok(None),
        Some([1]) => Ok(Some(true)),
        Some([0]) => Ok(Some(false)),
        Some(other) => Err(format!("{PRIVATE_NETWORK_KEY} holds {other:?}")),
    }
}

/// The grants, read through to each account's store once and then kept in memory: nothing
/// but this writes them.
#[derive(Default)]
pub(crate) struct AgentAppSettings {
    private_network: Mutex<HashMap<u64, Option<bool>>>,
}

impl AgentAppSettings {
    pub(crate) async fn private_network(
        &self,
        store: &dyn ConversationKv,
        cid: u64,
    ) -> Result<Option<bool>, String> {
        if let Some(known) = self.private_network.lock().get(&cid) {
            return Ok(*known);
        }
        let stored = decode(store.get(PRIVATE_NETWORK_KEY).await?)?;
        self.private_network.lock().insert(cid, stored);
        Ok(stored)
    }

    pub(crate) async fn set_private_network(
        &self,
        store: &dyn ConversationKv,
        cid: u64,
        on: bool,
    ) -> Result<(), String> {
        store.set(PRIVATE_NETWORK_KEY, vec![u8::from(on)]).await?;
        self.private_network.lock().insert(cid, Some(on));
        Ok(())
    }
}

/// The grants over each account's own bucket, as host egress reads them.
pub(crate) struct AccountGrants<'a> {
    pub settings: &'a AgentAppSettings,
    pub io: Arc<dyn HostIo>,
}

impl MemberGrants for AccountGrants<'_> {
    fn private_network(&self, cid: u64) -> BoxFuture<'_, Result<Option<bool>, String>> {
        Box::pin(async move {
            let store = HostKv::account(self.io.clone(), cid);
            self.settings.private_network(&store, cid).await
        })
    }
}

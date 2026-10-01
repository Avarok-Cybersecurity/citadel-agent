//! Starting the agent-hosted ILM when a client that asked for it joins a session.

use crate::kernel::ilm::HostIo;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::Ratchet;
use std::sync::Arc;
use uuid::Uuid;

impl<T, R> CitadelWorkspaceService<T, R>
where
    T: IOInterface + Sync,
    R: Ratchet,
{
    /// Whether `connection`'s client declared that the agent hosts its ILM.
    pub(crate) fn wants_agent_ilm(&self, connection: Uuid) -> bool {
        self.client_capabilities
            .read()
            .get(&connection)
            .is_some_and(|capabilities| capabilities.agent_ilm)
    }

    /// `connection` has just become a subscriber of `cid`. If its client hosts
    /// no ILM of its own, the agent hosts the account's; started before the
    /// caller is answered, so its first `SendReliable` finds it running.
    ///
    /// Once hosted, an account stays hosted for the life of its session.
    pub(crate) async fn host_ilm_for(&self, cid: u64, connection: Uuid) {
        if !self.wants_agent_ilm(connection) {
            return;
        }
        let io: Arc<dyn HostIo> = Arc::new(self.clone());
        if let Err(err) = self.ilm_hosts.ensure(cid, io).await {
            warn!(target: "citadel", "[ILM-HOST] {cid}: not hosted: {err}");
        }
    }
}

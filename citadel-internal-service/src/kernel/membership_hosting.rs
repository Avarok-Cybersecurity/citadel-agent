//! What a client declares it can do, and starting the agent-hosted ILM when a
//! client that asked for it joins a session.

use crate::kernel::ilm::HostIo;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    AgentCapabilities, ClientCapabilities, InternalServiceResponse,
};
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::Ratchet;
use std::sync::Arc;
use uuid::Uuid;

impl<T, R> CitadelWorkspaceService<T, R>
where
    T: IOInterface + Sync,
    R: Ratchet,
{
    /// Whether the agent keeps its hosted accounts' peers connected itself, so a window
    /// must not dial for them.
    pub(crate) fn supervises_p2p(&self) -> bool {
        self.supervisor.is_some_and(|policy| policy.dial_peers)
    }

    /// Whether `connection`'s client declared that the agent hosts its ILM.
    pub(crate) fn wants_agent_ilm(&self, connection: Uuid) -> bool {
        self.client_capabilities
            .read()
            .get(&connection)
            .is_some_and(|capabilities| capabilities.agent_ilm)
    }

    /// `connection`'s client says what it can do; answered with what the agent does.
    pub(crate) async fn declare(
        &self,
        connection: Uuid,
        capabilities: ClientCapabilities,
        request_id: Uuid,
    ) -> InternalServiceResponse {
        self.client_capabilities
            .write()
            .insert(connection, capabilities);
        // Sessions this connection already holds are hosted from now on.
        let held: Vec<u64> = self
            .server_connection_map
            .read()
            .iter()
            .filter(|(_, conn)| conn.subscribers.contains(connection))
            .map(|(cid, _)| *cid)
            .collect();
        for cid in held {
            self.host_ilm_for(cid, connection).await;
        }
        InternalServiceResponse::AgentCapabilities(AgentCapabilities {
            cid: 0,
            agent_ilm: true,
            multi_window: true,
            supervises_p2p: self.supervises_p2p(),
            notices_heard: self.notices.is_heard(),
            request_id: Some(request_id),
        })
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
        match self.ilm_hosts.ensure(cid, io).await {
            Ok(_) => {
                self.displace_older_pages(cid);
                self.supervise(cid);
            }
            Err(err) => warn!(target: "citadel", "[ILM-HOST] {cid}: not hosted: {err}"),
        }
    }
}

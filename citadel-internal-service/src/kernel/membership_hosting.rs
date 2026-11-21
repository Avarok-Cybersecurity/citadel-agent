//! What a client declares it can do, and starting the agent-hosted ILM when a
//! client joins a session.

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
        InternalServiceResponse::AgentCapabilities(AgentCapabilities {
            cid: 0,
            multi_window: true,
            supervises_p2p: self.supervises_p2p(),
            notices_heard: self.notices.is_heard(),
            ihttp: true,
            request_id: Some(request_id),
        })
    }

    /// A connection has just become a subscriber of `cid`: the agent hosts the
    /// account's ILM, whatever the connection declared. Started before the caller
    /// is answered, so its first `SendReliable` finds it running.
    ///
    /// Once hosted, an account stays hosted for the life of its session.
    pub(crate) async fn host_ilm_for(&self, cid: u64) {
        let io: Arc<dyn HostIo> = Arc::new(self.clone());
        match self.ilm_hosts.ensure(cid, io).await {
            Ok(_) => self.supervise(cid),
            Err(err) => warn!(target: "citadel", "[ILM-HOST] {cid}: not hosted: {err}"),
        }
    }
}

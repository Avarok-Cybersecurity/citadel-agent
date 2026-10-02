//! ILM's LocalDB requests, answered in-process.

use super::HostIo;
use async_trait::async_trait;
use citadel_internal_service_connector::messenger::backend::BackendChannel;
use citadel_internal_service_connector::messenger::WrappedMessage;
use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
use intersession_layer_messaging::BackendError;
use std::sync::Arc;
use uuid::Uuid;

/// The agent's [`BackendChannel`]: no socket, no reply matching, no deadline --
/// the agent answers its own store directly, as the browser's request would be
/// answered on arrival.
#[derive(Clone)]
pub(crate) struct AgentChannel {
    io: Arc<dyn HostIo>,
}

impl AgentChannel {
    pub(crate) fn new(io: Arc<dyn HostIo>) -> Self {
        Self { io }
    }
}

#[async_trait]
impl BackendChannel for AgentChannel {
    async fn request(
        &self,
        request: InternalServiceRequest,
        _request_id: Uuid,
    ) -> Result<InternalServiceResponse, BackendError<WrappedMessage>> {
        Ok(self.io.local_db(request).await)
    }
}

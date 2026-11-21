//! How a backend's LocalDB requests reach the agent's store.
//!
//! The agent, hosting ILM, answers them in-process with its own channel
//! (citadel-internal-service `kernel/ilm`). The storage logic in
//! [`CitadelWorkspaceBackend`](super::backend::CitadelWorkspaceBackend) is
//! independent of the channel, so it is tested here through a plain map.

use crate::messenger::WrappedMessage;
use async_trait::async_trait;
use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
use intersession_layer_messaging::BackendError;
use uuid::Uuid;

/// Answers one LocalDB (or Batched) request.
///
/// `Ok` is the agent's answer, whatever it says -- a refusal is an answer, and
/// the backend classifies it. `Err` means no answer will come.
#[async_trait]
pub trait BackendChannel: Clone + Send + Sync + 'static {
    async fn request(
        &self,
        request: InternalServiceRequest,
        request_id: Uuid,
    ) -> Result<InternalServiceResponse, BackendError<WrappedMessage>>;
}

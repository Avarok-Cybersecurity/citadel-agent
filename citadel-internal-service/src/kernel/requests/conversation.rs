//! Conversation requests, handed to the agent's conversation store.

use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::InternalServiceRequest;
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let response = this.conversations.answer(this, request).await;
    Some(HandledRequestResult { response, uuid })
}

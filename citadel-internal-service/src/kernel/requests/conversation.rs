//! Conversation requests, handed to the agent's conversation store.

use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    ConversationFailure, InternalServiceRequest, InternalServiceResponse,
};
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

/// A request this module answers.
pub fn is_request(request: &InternalServiceRequest) -> bool {
    matches!(
        request,
        InternalServiceRequest::ConversationSend { .. }
            | InternalServiceRequest::ConversationResend { .. }
            | InternalServiceRequest::ConversationEdit { .. }
            | InternalServiceRequest::ConversationDelete { .. }
            | InternalServiceRequest::ConversationReact { .. }
            | InternalServiceRequest::ConversationMarkRead { .. }
            | InternalServiceRequest::ConversationRecord { .. }
            | InternalServiceRequest::ConversationPatch { .. }
            | InternalServiceRequest::ConversationClear { .. }
            | InternalServiceRequest::ConversationList { .. }
            | InternalServiceRequest::ConversationPage { .. }
            | InternalServiceRequest::SetAccountPreferences { .. }
            | InternalServiceRequest::GetAccountPreferences { .. }
    )
}

/// The ownership gate's answer to one of them, with the gate's wording.
pub fn refusal(request: &InternalServiceRequest, reason: &str) -> Option<InternalServiceResponse> {
    if !is_request(request) {
        return None;
    }
    let cid = request.session_cid()?;
    Some(InternalServiceResponse::ConversationFailure(
        ConversationFailure {
            cid,
            message: reason.to_string(),
            request_id: request.request_id().copied(),
        },
    ))
}

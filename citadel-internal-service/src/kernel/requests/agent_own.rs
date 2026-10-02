//! Requests the agent makes for itself -- the account ILM it hosts (kernel/ilm)
//! reads and writes LocalDB and sends ILM frames -- answered by the same
//! handlers a window's requests are, without a window's ownership gate.

use super::{local_db, message};
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    BatchedResponseData, InternalServiceRequest, InternalServiceResponse, LocalDBGetKVFailure,
};
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

/// A LocalDB request answered for the agent itself -- the account ILM it
/// hosts (kernel/ilm) -- without the ownership gate, which exists to keep one
/// connection out of another's session and has no connection to judge here.
pub(crate) async fn answer_local_db<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    request: InternalServiceRequest,
) -> InternalServiceResponse {
    let request_id = request.request_id().copied();
    let answered = match request {
        InternalServiceRequest::LocalDBGetKV { .. } => {
            local_db::get_kv::handle(this, Uuid::nil(), request).await
        }
        InternalServiceRequest::LocalDBSetKV { .. } => {
            local_db::set_kv::handle(this, Uuid::nil(), request).await
        }
        InternalServiceRequest::LocalDBDeleteKV { .. } => {
            local_db::delete_kv::handle(this, Uuid::nil(), request).await
        }
        InternalServiceRequest::LocalDBGetAllKV { .. } => {
            local_db::get_all_kv::handle(this, Uuid::nil(), request).await
        }
        InternalServiceRequest::Batched {
            request_id,
            commands,
        } => {
            let mut results = Vec::with_capacity(commands.len());
            for command in commands {
                results.push(Box::pin(answer_local_db(this, command)).await);
            }
            return InternalServiceResponse::BatchedResponse(BatchedResponseData {
                cid: 0,
                request_id: Some(request_id),
                results,
            });
        }
        other => {
            return InternalServiceResponse::LocalDBGetKVFailure(LocalDBGetKVFailure {
                cid: other.session_cid().unwrap_or(0),
                peer_cid: None,
                message: "not a request the agent's own ILM makes".to_string(),
                request_id,
            })
        }
    };
    match answered {
        Some(result) => result.response,
        None => InternalServiceResponse::LocalDBGetKVFailure(LocalDBGetKVFailure {
            cid: 0,
            peer_cid: None,
            message: "the LocalDB handler gave no answer".to_string(),
            request_id,
        }),
    }
}

/// `Message`'s handler, for the agent's own ILM frames.
pub(crate) async fn send_message<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<InternalServiceResponse> {
    message::handle(this, uuid, request)
        .await
        .map(|result| result.response)
}

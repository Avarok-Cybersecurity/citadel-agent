//! `SendReliable`: a window's reliable message, through the ILM the agent hosts.
//!
//! The answer means "ILM has it": stored, numbered, and retransmitted until the
//! peer acknowledges. That is what a browser's own `send_p2p_message_reliable`
//! resolving meant, so a window that moves its sends here keeps its semantics.

use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_connector::messenger::CompressionHint;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, MessageSendFailure, SendReliableAccepted,
};
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::SendReliable {
        request_id,
        cid,
        peer_cid,
        message,
        security_level,
        compression_hint,
    } = request
    else {
        unreachable!("dispatched by variant")
    };
    let failure = |message: String| {
        Some(HandledRequestResult {
            response: InternalServiceResponse::MessageSendFailure(MessageSendFailure {
                cid,
                message,
                request_id: Some(request_id),
            }),
            uuid,
        })
    };

    // An unknown name is refused rather than ignored, as the WASM client does,
    // so a typo fails loudly instead of sending uncompressed forever.
    let hint = match compression_hint
        .as_deref()
        .map(str::parse::<CompressionHint>)
        .transpose()
    {
        Ok(hint) => hint,
        Err(err) => return failure(format!("Unknown compression hint: {err}")),
    };
    let Some(host) = this.ilm_hosts.get(cid) else {
        return failure(format!(
            "The agent does not host messaging for session {cid}; declare agent_ilm first"
        ));
    };
    match host
        .send(peer_cid, request_id, message, security_level, hint)
        .await
    {
        Ok(()) => Some(HandledRequestResult {
            response: InternalServiceResponse::SendReliableAccepted(SendReliableAccepted {
                cid,
                peer_cid,
                request_id: Some(request_id),
            }),
            uuid,
        }),
        Err(err) => failure(format!("ILM refused the message: {err}")),
    }
}

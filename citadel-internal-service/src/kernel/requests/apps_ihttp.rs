//! `IhttpSend`: one internal-HTTP frame to a peer, through the account's ILM
//! (kernel/apps/ihttp). Validated here and sent only to a peer that advertised it.

use crate::kernel::apps::ihttp;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_connector::messenger::CompressionHint;
use citadel_internal_service_types::{
    IhttpSendAccepted, IhttpSendFailure, IhttpSendRefusal, InternalServiceRequest,
    InternalServiceResponse, SecurityLevel,
};
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::IhttpSend {
        request_id,
        cid,
        peer_cid,
        frame,
    } = request
    else {
        unreachable!("dispatched by variant")
    };
    let refuse = |refusal| {
        Some(HandledRequestResult {
            response: InternalServiceResponse::IhttpSendFailure(IhttpSendFailure {
                cid,
                peer_cid,
                request_id: Some(request_id),
                refusal,
            }),
            uuid,
        })
    };
    if !this.attached(cid, uuid) {
        return refuse(IhttpSendRefusal::Session);
    }
    let Some(host) = this.ilm_hosts.get(cid) else {
        return refuse(IhttpSendRefusal::NotHosted);
    };
    let carries = host.peer_capabilities(peer_cid).carries_internal_http();
    let envelope = match ihttp::outbound(&frame, carries, &this.apps.ihttp) {
        Ok(envelope) => envelope,
        Err(refusal) => return refuse(refusal),
    };
    // A frame's body is whatever the app sends, often already compressed: never worth a codec.
    let sent = host
        .send(
            peer_cid,
            request_id,
            envelope,
            SecurityLevel::Standard,
            Some(CompressionHint::Opaque),
        )
        .await;
    match sent {
        Ok(()) => Some(HandledRequestResult {
            response: InternalServiceResponse::IhttpSendAccepted(IhttpSendAccepted {
                cid,
                peer_cid,
                request_id: Some(request_id),
            }),
            uuid,
        }),
        Err(why) => {
            warn!(target: "citadel", "[IHTTP] {cid}: ILM refused a frame for {peer_cid}: {why}");
            refuse(IhttpSendRefusal::Queue)
        }
    }
}

/// A window's `Message` or `SendReliable` whose bytes are an internal-HTTP envelope: refused,
/// because only `IhttpSend` checks the frame and the peer's advertisement.
pub(crate) fn smuggles_internal_http(command: &InternalServiceRequest) -> bool {
    match command {
        InternalServiceRequest::Message { message, .. }
        | InternalServiceRequest::SendReliable { message, .. } => ihttp::smuggled(message),
        _ => false,
    }
}

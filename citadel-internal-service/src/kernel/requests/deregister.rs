use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    DeregisterFailure, DeregisterSuccess, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{
    DeregisterFromHypernode, NodeRequest, NodeResult, Ratchet, VirtualTargetType,
};
use futures::StreamExt;
use std::time::Duration;
use uuid::Uuid;

/// How long to wait for the protocol to say whether the account is gone.
///
/// The alternative to a bound is a handler that never answers, which reaches
/// the user as the sign-out modal spinning forever.
const DEREGISTER_WAIT: Duration = Duration::from_secs(30);

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::Deregister { request_id, cid } = request else {
        unreachable!("Should never happen if programmed properly")
    };

    info!(target: "citadel", "Processing Deregister request for CID {cid}");

    let remote = this.remote();

    // Create the deregister request using the C2S connection type
    // This permanently removes the account from the server
    let request = NodeRequest::DeregisterFromHypernode(DeregisterFromHypernode {
        session_cid: cid,
        v_conn_type: VirtualTargetType::LocalGroupServer { session_cid: cid },
    });

    let failure = |message: String| {
        Some(HandledRequestResult {
            response: InternalServiceResponse::DeregisterFailure(DeregisterFailure {
                cid,
                message,
                request_id: Some(request_id),
            }),
            uuid,
        })
    };

    // `remote.send` resolves when the SDK's mpsc channel ACCEPTS the request --
    // before its node loop has dequeued it, as `file/upload.rs` documents at
    // length for the same call. This handler used to report DeregisterSuccess
    // on that, so "delete my account permanently" answered yes to a request
    // that had been queued and nothing more. CI measured the consequence
    // directly: `Deregister success: true` followed by `Can login after
    // deregister: true` -- the account was still there, and the person had been
    // told it was gone.
    //
    // The subscription carries the protocol's own answer, including a `success`
    // flag for a deregistration the server refused.
    let mut subscription = match remote.send_callback_subscription(request).await {
        Ok(subscription) => subscription,
        Err(err) => return failure(format!("Failed to deregister: {err:?}")),
    };

    let outcome = tokio::time::timeout(DEREGISTER_WAIT, async {
        while let Some(event) = subscription.next().await {
            if let NodeResult::DeRegistration(dereg) = event {
                return Some(dereg.success);
            }
        }
        None
    })
    .await;

    match outcome {
        Ok(Some(true)) => {
            // Removed only now. Removing it first -- which is what this did --
            // takes the session out of the map whatever the protocol decides,
            // so a REFUSED deregistration left a live SDK session with no entry
            // representing it: gone from the UI, still connected, unreachable.
            this.server_connection_map.write().remove(&cid);
            info!(target: "citadel", "Deregister successful for CID {cid}");
            Some(HandledRequestResult {
                response: InternalServiceResponse::DeregisterSuccess(DeregisterSuccess {
                    cid,
                    request_id: Some(request_id),
                }),
                uuid,
            })
        }
        Ok(Some(false)) => {
            warn!(target: "citadel", "Deregister refused by the protocol for CID {cid}");
            failure("The server refused to deregister this account".to_string())
        }
        Ok(None) => {
            warn!(target: "citadel", "Deregister subscription ended with no answer for CID {cid}");
            failure("The deregistration ended without an answer".to_string())
        }
        Err(_) => {
            warn!(target: "citadel", "Deregister timed out after {DEREGISTER_WAIT:?} for CID {cid}");
            failure(format!(
                "No answer to the deregistration within {DEREGISTER_WAIT:?}; the account may still exist"
            ))
        }
    }
}

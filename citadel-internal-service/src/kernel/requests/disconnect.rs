use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    DisconnectFailure, DisconnectNotification, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{DisconnectFromHypernode, NodeRequest, Ratchet};
use futures::StreamExt;
use uuid::Uuid;

/// Maximum number of polling attempts to verify session cleanup
const MAX_CLEANUP_POLL_ATTEMPTS: u32 = 10;

/// Initial delay between poll attempts (exponential backoff applied)
const INITIAL_POLL_DELAY_MS: u64 = 50;

/// Maximum delay between poll attempts
const MAX_POLL_DELAY_MS: u64 = 200;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::Disconnect { request_id, cid } = request else {
        unreachable!("Should never happen if programmed properly")
    };
    let remote = this.remote();

    let request =
        NodeRequest::DisconnectFromHypernode(DisconnectFromHypernode { session_cid: cid });

    this.server_connection_map.write().remove(&cid);

    match remote.send(request).await {
        Ok(_res) => {
            info!(target: "citadel", "[Disconnect] SDK disconnect request succeeded for cid={}", cid);

            // Poll SDK to verify the session is truly cleared before sending notification
            // This prevents race conditions where the client reconnects before SDK cleanup completes
            let mut backoff_ms = INITIAL_POLL_DELAY_MS;

            for attempt in 0..MAX_CLEANUP_POLL_ATTEMPTS {
                tokio::time::sleep(tokio::time::Duration::from_millis(backoff_ms)).await;

                // Query SDK for active sessions
                match remote
                    .send_callback_subscription(NodeRequest::GetActiveSessions)
                    .await
                {
                    Ok(mut stream) => {
                        if let Some(result) = stream.next().await {
                            // Check if the result is a session list that still contains our CID
                            // The result is logged as Debug, so we check the string representation
                            let result_str = format!("{:?}", result);
                            let cid_str = format!("{}", cid);

                            // If the CID is not found in the session list, cleanup is complete
                            if !result_str.contains(&cid_str) {
                                info!(target: "citadel", "[Disconnect] CID {} confirmed cleared from SDK after {} attempts", cid, attempt + 1);
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        warn!(target: "citadel", "[Disconnect] Failed to query SDK sessions (attempt {}): {:?}", attempt + 1, e);
                    }
                }

                // Exponential backoff, capped at MAX_POLL_DELAY_MS
                backoff_ms = (backoff_ms * 2).min(MAX_POLL_DELAY_MS);
            }

            // Now safe to send DisconnectNotification to client
            let disconnect_success =
                InternalServiceResponse::DisconnectNotification(DisconnectNotification {
                    cid,
                    peer_cid: None,
                    request_id: Some(request_id),
                });
            Some(HandledRequestResult {
                response: disconnect_success,
                uuid,
            })
        }
        Err(err) => {
            let error_message = format!("Failed to disconnect {err:?}");
            info!(target: "citadel", "{error_message}");
            let disconnect_failure =
                InternalServiceResponse::DisconnectFailure(DisconnectFailure {
                    cid,
                    message: error_message,
                    request_id: Some(request_id),
                });
            Some(HandledRequestResult {
                response: disconnect_failure,
                uuid,
            })
        }
    }
}

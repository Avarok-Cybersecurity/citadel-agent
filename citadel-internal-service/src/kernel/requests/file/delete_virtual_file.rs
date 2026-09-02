use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    DeleteVirtualFileFailure, DeleteVirtualFileSuccess, InternalServiceRequest,
    InternalServiceResponse,
};
use citadel_sdk::logging::{error, warn};
use citadel_sdk::prelude::{
    DeleteObject, NetworkError, NodeRequest, NodeResult, Ratchet, VirtualTargetType,
};
use futures::{Stream, StreamExt};
use std::time::Duration;
use uuid::Uuid;

/// How long to wait for the REVFS ack that says the delete actually ran.
///
/// Thirty seconds matches `DEREGISTER_WAIT` in `deregister.rs` — the crate's
/// convention for bounding a callback subscription whose answer may never
/// come (a dropped link, a peer that goes away mid-operation). The bound is
/// what lets the handler answer "unknown" honestly instead of hanging.
const DELETE_WAIT: Duration = Duration::from_secs(30);

/// What actually happened to the delete, as told by the protocol — separated
/// from the handler's response construction so it is testable without a node.
enum DeleteOutcome {
    /// The REVFS ack arrived with no error: the file is gone on the target.
    Deleted,
    /// The REVFS ack arrived carrying the remote's error message.
    Rejected(String),
    /// The kernel dropped the subscription without an ack.
    Ended,
    /// No ack within `DELETE_WAIT`.
    TimedOut,
}

/// Drives the callback subscription to the delete's terminal event.
///
/// `remote.send` resolves when the SDK's mpsc channel ACCEPTS the request —
/// before the node loop has even dequeued it (see `file/upload.rs` and the
/// same trap documented in `deregister.rs`). This handler used to report
/// `DeleteVirtualFileSuccess` on that acceptance, so "delete my encrypted
/// file" answered yes while the file could still exist. The real completion
/// signal is the REVFS ack, delivered as `NodeResult::ReVFS` with the
/// remote's error (if any) — the same event the SDK's own
/// `remote_encrypted_virtual_filesystem_delete` waits for.
async fn await_delete_outcome<R: Ratchet, S>(events: &mut S, wait: Duration) -> DeleteOutcome
where
    S: Stream<Item = NodeResult<R>> + Unpin,
{
    let ack = tokio::time::timeout(wait, async {
        while let Some(evt) = events.next().await {
            if let NodeResult::ReVFS(result) = evt {
                return Some(result.error_message);
            }
        }
        None
    })
    .await;

    match ack {
        Ok(Some(None)) => DeleteOutcome::Deleted,
        Ok(Some(Some(error_message))) => DeleteOutcome::Rejected(error_message),
        Ok(None) => DeleteOutcome::Ended,
        Err(_elapsed) => DeleteOutcome::TimedOut,
    }
}

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::DeleteVirtualFile {
        virtual_directory,
        cid,
        peer_cid,
        request_id,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };
    let remote = this.remote();

    // Extract what we need from the lock, then drop it before any await
    let delete_request: Result<NodeRequest, NetworkError> = {
        let lock = this.server_connection_map.read();
        match lock.get(&cid) {
            Some(conn) => {
                if let Some(peer_cid) = peer_cid {
                    if conn.peers.contains_key(&peer_cid) {
                        Ok(NodeRequest::DeleteObject(DeleteObject {
                            v_conn: VirtualTargetType::LocalGroupPeer {
                                session_cid: cid,
                                peer_cid,
                            },
                            virtual_dir: virtual_directory,
                            security_level: Default::default(),
                        }))
                    } else {
                        Err(NetworkError::msg("Peer Connection Not Found"))
                    }
                } else {
                    Ok(NodeRequest::DeleteObject(DeleteObject {
                        v_conn: VirtualTargetType::LocalGroupServer { session_cid: cid },
                        virtual_dir: virtual_directory,
                        security_level: Default::default(),
                    }))
                }
            }
            None => {
                error!(target: "citadel","delete_virtual_file: server connection not found");
                Err(NetworkError::msg(
                    "delete_virtual_file: Server Connection Not Found",
                ))
            }
        }
    }; // Lock dropped here - BEFORE any await

    let failure = |message: String| {
        InternalServiceResponse::DeleteVirtualFileFailure(DeleteVirtualFileFailure {
            cid,
            message,
            request_id: Some(request_id),
        })
    };

    let response = match delete_request {
        // Success is reported only once the REVFS ack confirms the delete
        // ran; see `await_delete_outcome` for why `remote.send` alone proved
        // nothing.
        Ok(request) => match remote.send_callback_subscription(request).await {
            Ok(mut subscription) => {
                match await_delete_outcome(&mut subscription, DELETE_WAIT).await {
                    DeleteOutcome::Deleted => InternalServiceResponse::DeleteVirtualFileSuccess(
                        DeleteVirtualFileSuccess {
                            cid,
                            request_id: Some(request_id),
                        },
                    ),
                    DeleteOutcome::Rejected(err) => failure(err),
                    DeleteOutcome::Ended => {
                        failure("The delete ended without an answer from the protocol".to_string())
                    }
                    DeleteOutcome::TimedOut => {
                        warn!(target: "citadel", "DeleteVirtualFile for CID {cid} received no REVFS ack within {DELETE_WAIT:?}");
                        failure(format!(
                            "No answer to the delete within {DELETE_WAIT:?}; the file may still exist"
                        ))
                    }
                }
            }
            Err(err) => failure(err.into_string()),
        },
        Err(err) => failure(err.into_string()),
    };

    Some(HandledRequestResult { response, uuid })
}

#[cfg(test)]
mod tests {
    use super::*;
    use citadel_sdk::prelude::{
        GroupBroadcast, GroupEvent, MessageGroupKey, ReVFSResult, StackedRatchet, Ticket,
    };

    fn revfs_ack(error_message: Option<&str>) -> NodeResult<StackedRatchet> {
        NodeResult::ReVFS(ReVFSResult {
            error_message: error_message.map(str::to_string),
            data: None,
            ticket: Ticket(1),
            session_cid: 1,
        })
    }

    /// An event the wait must skip over rather than treat as the ack.
    fn non_terminal_event() -> NodeResult<StackedRatchet> {
        NodeResult::GroupEvent(GroupEvent {
            session_cid: 1,
            ticket: Ticket(1),
            event: GroupBroadcast::LeaveRoom {
                key: MessageGroupKey::new(1, 1),
            },
        })
    }

    /// The M3 defect distilled: success may only be claimed after an ack with
    /// no error has actually been observed — never before, and never for an
    /// ack that carries an error.
    #[tokio::test]
    async fn success_requires_a_clean_ack() {
        let mut events = futures::stream::iter(vec![non_terminal_event(), revfs_ack(None)]);
        let outcome = await_delete_outcome(&mut events, Duration::from_secs(5)).await;
        assert!(matches!(outcome, DeleteOutcome::Deleted));
    }

    /// An ack carrying the remote's error message is a rejection, verbatim.
    #[tokio::test]
    async fn error_ack_is_a_rejection() {
        let mut events = futures::stream::iter(vec![revfs_ack(Some("no such file"))]);
        match await_delete_outcome(&mut events, Duration::from_secs(5)).await {
            DeleteOutcome::Rejected(message) => assert_eq!(message, "no such file"),
            _ => panic!("an ack with an error must not be reported as a delete"),
        }
    }

    /// No ack at all must resolve to TimedOut within the bound — not hang,
    /// and not count as success. The outer timeout is the test's own guard:
    /// with an unbounded wait the inner future never resolves and this fails
    /// at the `expect`.
    #[tokio::test]
    async fn missing_ack_times_out_as_failure() {
        let mut events = futures::stream::pending::<NodeResult<StackedRatchet>>();
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            await_delete_outcome(&mut events, Duration::from_millis(100)),
        )
        .await
        .expect("the wait must resolve on its own");
        assert!(matches!(outcome, DeleteOutcome::TimedOut));
    }

    /// A subscription dropped without an ack is an unknown outcome, reported
    /// as failure — the file's fate was never confirmed.
    #[tokio::test]
    async fn ended_subscription_is_not_a_delete() {
        let mut events = futures::stream::iter(Vec::<NodeResult<StackedRatchet>>::new());
        let outcome = await_delete_outcome(&mut events, Duration::from_secs(5)).await;
        assert!(matches!(outcome, DeleteOutcome::Ended));
    }
}

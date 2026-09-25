//! Peer messaging assertions shared by the suites that interleave messages
//! with file transfers (`file_transfer.rs`, `an_unanswered_offer_*`).
//!
//! One copy, because the question they answer -- "did this exact message reach
//! the peer, ignoring the transfer's own bookkeeping" -- must be asked the same
//! way everywhere it is asked.

use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, MessageNotification,
};
use uuid::Uuid;

/// Sends one P2P message and asserts the peer receives exactly it.
///
/// `context` names what a failure means, because the call sites fail for
/// different reasons and a shared "message not received" would say none of
/// them.
#[allow(clippy::too_many_arguments)]
pub async fn send_and_expect_message(
    to_service_a: &tokio::sync::mpsc::UnboundedSender<InternalServiceRequest>,
    from_service_a: &mut tokio::sync::mpsc::UnboundedReceiver<InternalServiceResponse>,
    from_service_b: &mut tokio::sync::mpsc::UnboundedReceiver<InternalServiceResponse>,
    cid_a: u64,
    cid_b: u64,
    body: &[u8],
    context: &str,
) {
    let message = Vec::from(body);
    to_service_a
        .send(InternalServiceRequest::Message {
            message: message.clone(),
            cid: cid_a,
            peer_cid: Some(cid_b),
            security_level: Default::default(),
            request_id: Uuid::new_v4(),
        })
        .unwrap();

    let send_response = next_ignoring_transfer_noise(from_service_a, 30).await;
    assert!(
        matches!(
            send_response,
            Some(InternalServiceResponse::MessageSendSuccess(..))
        ),
        "the send itself was refused ({context}): {send_response:?}"
    );

    // Bounded: the defect this covers presents as silence, and an
    // unbounded recv() would hang the suite rather than fail it.
    let notification = next_ignoring_transfer_noise(from_service_b, 30)
        .await
        .unwrap_or_else(|| panic!("no message reached the peer within 30s -- {context}"));

    match notification {
        InternalServiceResponse::MessageNotification(MessageNotification {
            message: received,
            ..
        }) => assert_eq!(&*message, &*received, "the peer received different bytes"),
        other => panic!("expected a MessageNotification, got {other:?} -- {context}"),
    }
}

/// The next response that is not file-transfer bookkeeping.
///
/// A transfer leaves ticks and status notifications queued on BOTH sides,
/// and this test is about messages. Only those two variants are skipped --
/// anything else is returned so a real wrong-response failure still shows.
pub async fn next_ignoring_transfer_noise(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<InternalServiceResponse>,
    seconds: u64,
) -> Option<InternalServiceResponse> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(seconds);
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Err(_) => return None,
            Ok(None) => return None,
            Ok(Some(
                InternalServiceResponse::FileTransferTickNotification(..)
                | InternalServiceResponse::FileTransferStatusNotification(..),
            )) => continue,
            Ok(Some(other)) => return Some(other),
        }
    }
}

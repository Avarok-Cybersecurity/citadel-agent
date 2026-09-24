//! A peer file offer the recipient has not accepted must not stop the sender's
//! messages from reaching them.
//!
//! Found live: after "Send file" in the P2P chat, the sender's messages, call
//! invites and even ILM acknowledgements stopped reaching the recipient until
//! both pages reloaded. The sender's agent kept logging `sink.send() SUCCEEDED`;
//! the recipient's never logged a single receipt from that peer again.
//!
//! The SDK draws a peer file transfer's first group id from the same counter as
//! messages, and the recipient's ordered channel is sequenced by that id. It was
//! told to step over the id only when the transfer's first GROUP header arrived
//! -- which the sender does not send until the recipient ACCEPTS. So while an
//! offer sat unanswered, and for ever once it was declined, every later message
//! waited behind an id that was never going to arrive. The rekey messages travel
//! the same channel, so the ratchet wedged too.
//!
//! The existing message-after-transfer tests all use a REVFS push, which the
//! agent auto-accepts: the group header arrives at once and the gap closes. The
//! ordinary user-facing offer is the case none of them covered.

use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::peer_messaging::{next_ignoring_transfer_noise, send_and_expect_message};
    use crate::common::{get_free_port, register_and_connect_to_server_then_peers};
    use citadel_internal_service_types::{
        FileSource, FileTransferRequestNotification, InternalServiceRequest,
        InternalServiceResponse,
    };
    use citadel_sdk::prelude::*;
    use std::error::Error;
    use std::net::SocketAddr;
    use uuid::Uuid;

    /// What the recipient does with the offer before the next message is sent.
    #[derive(Clone, Copy, Debug)]
    enum Answer {
        /// Nothing: the offer is on screen and the user has not clicked yet.
        None,
        /// Declined. The sender never streams a group, so no group header will
        /// ever close the gap -- the permanent form of the stall.
        Decline,
    }

    async fn message_after_offer(answer: Answer) -> Result<(), Box<dyn Error>> {
        crate::common::setup_log();
        let bind_a: SocketAddr = format!("127.0.0.1:{}", get_free_port()).parse()?;
        let bind_b: SocketAddr = format!("127.0.0.1:{}", get_free_port()).parse()?;

        let mut peers = register_and_connect_to_server_then_peers::<StackedRatchet>(
            vec![bind_a, bind_b],
            None,
            None,
        )
        .await?;
        let (peer_one, peer_two) = peers.as_mut_slice().split_at_mut(1_usize);
        let (to_service_a, from_service_a, cid_a) = peer_one.get_mut(0_usize).unwrap();
        let (to_service_b, from_service_b, cid_b) = peer_two.get_mut(0_usize).unwrap();

        // Without this, a failure below cannot tell "the offer broke messaging"
        // from "messaging never worked in this fixture".
        send_and_expect_message(
            to_service_a,
            from_service_a,
            from_service_b,
            *cid_a,
            *cid_b,
            b"before the offer",
            "messaging was already broken before any offer",
        )
        .await;

        // The browser's "Send file": inline bytes, a plain FileTransfer.
        to_service_a.send(InternalServiceRequest::SendFile {
            request_id: Uuid::new_v4(),
            source: FileSource::ByteContents {
                file_name: "offer.txt".to_string(),
                data: b"an offer nobody has answered".to_vec(),
            },
            cid: *cid_a,
            transfer_type: TransferType::FileTransfer,
            peer_cid: Some(*cid_b),
            chunk_size: None,
        })?;
        let queued = next_ignoring_transfer_noise(from_service_a, 30).await;
        assert!(
            matches!(
                queued,
                Some(InternalServiceResponse::SendFileRequestSuccess(..))
            ),
            "the offer itself was refused: {queued:?}"
        );

        // The offer reaches the recipient -- this part always worked.
        let offer = next_ignoring_transfer_noise(from_service_b, 30).await;
        let Some(InternalServiceResponse::FileTransferRequestNotification(
            FileTransferRequestNotification { metadata, .. },
        )) = offer
        else {
            panic!("the recipient never saw the offer: {offer:?}");
        };

        if let Answer::Decline = answer {
            to_service_b.send(InternalServiceRequest::RespondFileTransfer {
                cid: *cid_b,
                peer_cid: *cid_a,
                object_id: metadata.object_id as _,
                accept: false,
                download_location: None,
                request_id: Uuid::new_v4(),
            })?;
        }

        send_and_expect_message(
            to_service_a,
            from_service_a,
            from_service_b,
            *cid_a,
            *cid_b,
            b"after the offer",
            &format!(
                "a peer file offer answered with {answer:?} stopped the sender's messages: \
                 the recipient's ordered channel is waiting for the group id the offer consumed"
            ),
        )
        .await;

        Ok(())
    }

    #[tokio::test]
    async fn a_message_after_an_offer_not_yet_answered_still_arrives() -> Result<(), Box<dyn Error>>
    {
        message_after_offer(Answer::None).await
    }

    #[tokio::test]
    async fn a_message_after_a_declined_offer_still_arrives() -> Result<(), Box<dyn Error>> {
        message_after_offer(Answer::Decline).await
    }
}

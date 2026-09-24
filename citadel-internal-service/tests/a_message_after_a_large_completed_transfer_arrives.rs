//! A message sent after a MULTI-group peer transfer has finished must arrive.
//!
//! A peer file transfer takes only its FIRST group id from the counter messages
//! use; the ids of its later groups are reserved on the file-transfer counter.
//! So the second group of a transfer that started at id S is S+1 -- the id the
//! very next message is also given. If the recipient treats every transfer group
//! header as "this id is not a message" and steps over it, the message that
//! really is S+1, sent once the transfer is done, arrives after the step and is
//! dropped as a duplicate. The sender's send reports success; nothing arrives.
//!
//! The sibling test in `file_transfer.rs` sends its message straight after the
//! transfer is QUEUED, so the message wins the race against the second group
//! header and the defect never shows. This one waits for both ends to report the
//! transfer complete first, which is what a person does.

use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::peer_messaging::{next_ignoring_transfer_noise, send_and_expect_message};
    use crate::common::{get_free_port, register_and_connect_to_server_then_peers};
    use citadel_internal_service_types::{
        FileSource, FileTransferTickNotification, InternalServiceRequest, InternalServiceResponse,
    };
    use citadel_sdk::prelude::*;
    use std::error::Error;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::sync::mpsc::UnboundedReceiver;
    use uuid::Uuid;

    /// Drains one side's ticks until the transfer reports `terminal`.
    async fn wait_for(
        rx: &mut UnboundedReceiver<InternalServiceResponse>,
        terminal: fn(&ObjectTransferStatus) -> bool,
        side: &str,
    ) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(InternalServiceResponse::FileTransferTickNotification(
                    FileTransferTickNotification { status, .. },
                ))) if terminal(&status) => return,
                Ok(Some(_)) => continue,
                Ok(None) => panic!("{side}'s response stream closed before the transfer finished"),
                Err(_) => panic!("{side} never reported the transfer finished within 60s"),
            }
        }
    }

    #[tokio::test]
    async fn a_message_after_a_completed_multi_group_transfer_still_arrives(
    ) -> Result<(), Box<dyn Error>> {
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
        let (_to_service_b, from_service_b, cid_b) = peer_two.get_mut(0_usize).unwrap();

        send_and_expect_message(
            to_service_a,
            from_service_a,
            from_service_b,
            *cid_a,
            *cid_b,
            b"before the transfer",
            "messaging was already broken before the transfer",
        )
        .await;

        // Four groups: a megabyte at 256 KiB per group. Say the group size
        // outright -- the SDK's default is 3 MiB, so a megabyte on the default is
        // ONE group and cannot show this at all. A REVFS push is auto-accepted by
        // the recipient's agent, so no answer is needed.
        to_service_a.send(InternalServiceRequest::SendFile {
            request_id: Uuid::new_v4(),
            source: FileSource::ByteContents {
                file_name: "large.bin".to_string(),
                data: vec![9u8; 1024 * 1024],
            },
            cid: *cid_a,
            transfer_type: TransferType::RemoteEncryptedVirtualFilesystem {
                virtual_path: PathBuf::from("/vfs/large-completed.bin"),
                security_level: Default::default(),
            },
            peer_cid: Some(*cid_b),
            chunk_size: Some(256 * 1024),
        })?;
        let queued = next_ignoring_transfer_noise(from_service_a, 30).await;
        assert!(
            matches!(
                queued,
                Some(InternalServiceResponse::SendFileRequestSuccess(..))
            ),
            "the transfer itself was refused: {queued:?}"
        );

        wait_for(
            from_service_b,
            |s| matches!(s, ObjectTransferStatus::ReceptionComplete),
            "the recipient",
        )
        .await;

        send_and_expect_message(
            to_service_a,
            from_service_a,
            from_service_b,
            *cid_a,
            *cid_b,
            b"after the completed transfer",
            "the message after a finished multi-group transfer was dropped: the recipient \
             stepped over a later group's id, which is the id this message was given",
        )
        .await;

        Ok(())
    }
}

use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::{
        exhaust_stream_to_file_completion, get_free_port, register_and_connect_to_server,
        register_and_connect_to_server_then_peers, server_info_file_transfer,
        RegisterAndConnectItems,
    };
    use citadel_internal_service::kernel::CitadelWorkspaceService;
    use citadel_internal_service_types::{
        DeleteVirtualFileSuccess, DownloadFileFailure, DownloadFileSuccess, FileSource,
        FileTransferRequestNotification, FileTransferStatusNotification,
        FileTransferTickNotification, InternalServiceRequest, InternalServiceResponse,
        MessageNotification, SendFileRequestFailure, SendFileRequestSuccess,
    };
    use citadel_sdk::logging::info;
    use citadel_sdk::prelude::*;
    use core::panic;
    use std::error::Error;
    use std::net::SocketAddr;
    use std::panic::{set_hook, take_hook};
    use std::path::PathBuf;
    use std::process::exit;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::mpsc::UnboundedReceiver;
    use uuid::Uuid;

    /// Drains one service's REVFS tick stream to its terminal status,
    /// asserting every tick carries the REQUESTING CLIENT's request_id.
    ///
    /// This is the wire contract the browser depends on: `PullObject` /
    /// `SendObject` cannot carry a request id, so the kernel threads it
    /// through `kernel/revfs_correlation.rs` (registered by the DownloadFile /
    /// SendFile handlers, reclaimed when the ObjectTransferHandle arrives).
    /// Before that registry, ticks fell back to the TCP-connection uuid and
    /// the client's correlation matched nothing — every completed REVFS
    /// download reported failure after its 30s timeout, and a REVFS upload
    /// had no completion signal at all.
    ///
    /// When `cmp_path` is given, the terminal is ReceptionComplete and the
    /// received file must match its contents; otherwise the terminal is the
    /// sender's TransferComplete.
    async fn exhaust_revfs_ticks_asserting_request_id(
        svc: &mut UnboundedReceiver<InternalServiceResponse>,
        expected_request_id: Uuid,
        cmp_path: Option<PathBuf>,
    ) {
        let mut received_path = None;
        loop {
            let response = svc.recv().await.unwrap();
            let InternalServiceResponse::FileTransferTickNotification(
                FileTransferTickNotification {
                    status, request_id, ..
                },
            ) = response
            else {
                // Same tolerance as exhaust_stream_to_file_completion: other
                // signals may interleave; only the tick stream is under test.
                citadel_sdk::logging::warn!(target: "citadel", "Unexpected signal {response:?}");
                continue;
            };
            assert_eq!(
                request_id,
                Some(expected_request_id),
                "REVFS ticks must carry the requesting client's request_id;                  the TCP-uuid fallback matches nothing client-side"
            );
            match status {
                ObjectTransferStatus::ReceptionBeginning(path, _) => received_path = Some(path),
                ObjectTransferStatus::TransferComplete => {
                    assert!(
                        cmp_path.is_none(),
                        "Expected a reception terminal, got the sender's TransferComplete"
                    );
                    return;
                }
                ObjectTransferStatus::ReceptionComplete => {
                    let cmp = cmp_path.expect("Expected a sender terminal, got ReceptionComplete");
                    let cmp_data = tokio::fs::read(cmp).await.unwrap();
                    let streamed_data =
                        tokio::fs::read(received_path.expect("No ReceptionBeginning tick"))
                            .await
                            .unwrap();
                    assert_eq!(
                        cmp_data.as_slice(),
                        streamed_data.as_slice(),
                        "Pulled file does not match the original"
                    );
                    return;
                }
                ObjectTransferStatus::Fail(err) => panic!("REVFS transfer failed: {err}"),
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn test_internal_service_standard_file_transfer_c2s() -> Result<(), Box<dyn Error>> {
        // Causes panics in spawned threads to be caught
        let orig_hook = take_hook();
        set_hook(Box::new(move |panic_info| {
            orig_hook(panic_info);
            exit(1);
        }));

        crate::common::setup_log();
        info!(target: "citadel", "above server spawn");
        let bind_address_internal_service: SocketAddr =
            format!("127.0.0.1:{}", get_free_port()).parse().unwrap();

        // TCP client (GUI, CLI) -> Internal Service -> Receiver File Transfer Kernel server
        let server_success = &Arc::new(AtomicBool::new(false));
        //let (server, server_bind_address) = server_info_file_transfer(server_success.clone());
        let (server, server_bind_address) =
            server_info_file_transfer::<StackedRatchet>(server_success.clone());

        tokio::task::spawn(server);

        info!(target: "citadel", "sub server spawn");
        let internal_service_kernel =
            CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(bind_address_internal_service)
                .await?;
        let internal_service = NodeBuilder::default()
            .with_backend(BackendType::Filesystem("filesystem".into()))
            .with_node_type(NodeType::Peer)
            .with_insecure_skip_cert_verification()
            .build(internal_service_kernel)?;

        tokio::task::spawn(internal_service);

        // give time for both the server and internal service to run

        tokio::time::sleep(Duration::from_millis(2000)).await;

        info!(target: "citadel", "about to connect to internal service");

        let to_spawn = vec![RegisterAndConnectItems {
            internal_service_addr: bind_address_internal_service,
            server_addr: server_bind_address,
            full_name: "John Doe",
            username: "john.doe",
            password: "secret",
            pre_shared_key: None::<PreSharedKey>,
        }];
        let returned_service_info = register_and_connect_to_server(to_spawn).await;
        let mut service_vec = returned_service_info.unwrap();
        if let Some((to_service, from_service, cid)) = service_vec.get_mut(0_usize) {
            let cmp_path = PathBuf::from("../resources/test.txt");

            let file_transfer_command = InternalServiceRequest::SendFile {
                request_id: Uuid::new_v4(),
                source: FileSource::Path(cmp_path.clone()),
                cid: *cid,
                transfer_type: TransferType::FileTransfer,
                peer_cid: None,
                chunk_size: None,
            };
            to_service.send(file_transfer_command).unwrap();
            exhaust_stream_to_file_completion(cmp_path, from_service).await;

            Ok(())
        } else {
            panic!("Service Spawn Error")
        }
    }

    #[tokio::test]
    async fn test_internal_service_peer_standard_file_transfer() -> Result<(), Box<dyn Error>> {
        crate::common::setup_log();
        // internal service for peer A
        let bind_address_internal_service_a: SocketAddr =
            format!("127.0.0.1:{}", get_free_port()).parse().unwrap();
        // internal service for peer B
        let bind_address_internal_service_b: SocketAddr =
            format!("127.0.0.1:{}", get_free_port()).parse().unwrap();

        let mut peer_return_handle_vec =
            register_and_connect_to_server_then_peers::<StackedRatchet>(
                vec![
                    bind_address_internal_service_a,
                    bind_address_internal_service_b,
                ],
                None,
                None,
            )
            .await?;

        let (peer_one, peer_two) = peer_return_handle_vec.as_mut_slice().split_at_mut(1_usize);
        let (to_service_a, from_service_a, cid_a) = peer_one.get_mut(0_usize).unwrap();
        let (to_service_b, from_service_b, cid_b) = peer_two.get_mut(0_usize).unwrap();

        let file_to_send = PathBuf::from("../resources/test.txt");

        let send_file_to_service_b_payload = InternalServiceRequest::SendFile {
            request_id: Uuid::new_v4(),
            source: FileSource::Path(file_to_send),
            cid: *cid_a,
            transfer_type: TransferType::FileTransfer,
            peer_cid: Some(*cid_b),
            chunk_size: None,
        };
        to_service_a.send(send_file_to_service_b_payload).unwrap();
        info!(target:"citadel", "File Transfer Request Sent from {cid_a:?}");

        info!(target:"citadel", "File Transfer Request Sent Successfully {cid_a:?}");
        let deserialized_service_b_payload_response = from_service_b.recv().await.unwrap();
        if let InternalServiceResponse::FileTransferRequestNotification(
            FileTransferRequestNotification { metadata, .. },
        ) = deserialized_service_b_payload_response
        {
            info!(target:"citadel", "File Transfer Request {cid_b:?}");

            let file_transfer_accept = InternalServiceRequest::RespondFileTransfer {
                cid: *cid_b,
                peer_cid: *cid_a,
                object_id: metadata.object_id as _,
                accept: true,
                download_location: None,
                request_id: Uuid::new_v4(),
            };
            to_service_b.send(file_transfer_accept).unwrap();
            info!(target:"citadel", "Accepted File Transfer {cid_b:?}");

            let file_transfer_accept = from_service_b.recv().await.unwrap();
            if let InternalServiceResponse::FileTransferStatusNotification(
                FileTransferStatusNotification {
                    cid: _,
                    object_id: _,
                    success,
                    response,
                    message: _,
                    request_id: _,
                },
            ) = file_transfer_accept
            {
                if success && response {
                    info!(target:"citadel", "File Transfer Accept Success {cid_b:?}");
                    // continue to status ticks
                } else {
                    panic!("Service B Accept Response Failure - Success: {success:?} Response {response:?}")
                }
            } else {
                panic!("Unhandled Service B response")
            }

            // Exhaust the stream for the receiver
            exhaust_stream_to_file_completion(
                PathBuf::from("../resources/test.txt"),
                from_service_b,
            )
            .await;
            // Exhaust the stream for the sender
            exhaust_stream_to_file_completion(
                PathBuf::from("../resources/test.txt"),
                from_service_a,
            )
            .await;
        } else {
            panic!("File Transfer P2P Failure");
        };

        Ok(())
    }

    #[tokio::test]
    async fn test_internal_service_c2s_revfs() -> Result<(), Box<dyn Error>> {
        crate::common::setup_log();
        info!(target: "citadel", "above server spawn");
        let bind_address_internal_service: SocketAddr =
            format!("127.0.0.1:{}", get_free_port()).parse().unwrap();

        // TCP client (GUI, CLI) -> Internal Service -> Receiver File Transfer Kernel server
        let server_success = &Arc::new(AtomicBool::new(false));
        let (server, server_bind_address) =
            server_info_file_transfer::<StackedRatchet>(server_success.clone());

        tokio::task::spawn(server);

        info!(target: "citadel", "sub server spawn");
        let internal_service_kernel =
            CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(bind_address_internal_service)
                .await?;

        let internal_service = NodeBuilder::default()
            .with_backend(BackendType::Filesystem("filesystem".into()))
            .with_node_type(NodeType::Peer)
            .with_insecure_skip_cert_verification()
            .build(internal_service_kernel)?;

        tokio::task::spawn(internal_service);

        // give time for both the server and internal service to run

        tokio::time::sleep(Duration::from_millis(2000)).await;

        info!(target: "citadel", "about to connect to internal service");

        let to_spawn = vec![RegisterAndConnectItems {
            internal_service_addr: bind_address_internal_service,
            server_addr: server_bind_address,
            full_name: "John Doe",
            username: "john.doe",
            password: "secret",
            pre_shared_key: None::<PreSharedKey>,
        }];
        let returned_service_info = register_and_connect_to_server(to_spawn).await;
        let mut service_vec = returned_service_info.unwrap();
        if let Some((to_service, from_service, cid)) = service_vec.get_mut(0_usize) {
            // Push file to REVFS
            let file_to_send = PathBuf::from("../resources/test.txt");
            let virtual_path = PathBuf::from("/vfs/test.txt");
            let push_request_id = Uuid::new_v4();
            let file_transfer_command = InternalServiceRequest::SendFile {
                request_id: push_request_id,
                source: FileSource::Path(file_to_send.clone()),
                cid: *cid,
                transfer_type: TransferType::RemoteEncryptedVirtualFilesystem {
                    virtual_path: virtual_path.clone(),
                    security_level: Default::default(),
                },
                peer_cid: None,
                chunk_size: None,
            };
            to_service.send(file_transfer_command).unwrap();
            let file_transfer_response = from_service.recv().await.unwrap();
            if let InternalServiceResponse::SendFileRequestFailure(SendFileRequestFailure {
                cid: _,
                message,
                request_id: _,
            }) = file_transfer_response
            {
                panic!("Send File Failure: {message:?}")
            }

            // Wait for the sender to complete the transfer. The Sender ticks
            // are the uploader's only real completion signal
            // (SendFileRequestSuccess above just means "queued") and must be
            // correlated to the SendFile request_id — c2s pushes register
            // under the session's own cid in kernel/revfs_correlation.rs.
            exhaust_revfs_ticks_asserting_request_id(from_service, push_request_id, None).await;

            // Download/Pull file from REVFS - Don't delete on pull
            let download_request_id = Uuid::new_v4();
            let file_download_command = InternalServiceRequest::DownloadFile {
                virtual_directory: virtual_path.clone(),
                security_level: None,
                delete_on_pull: false,
                cid: *cid,
                peer_cid: None,
                request_id: download_request_id,
            };
            to_service.send(file_download_command).unwrap();
            let download_file_response = from_service.recv().await.unwrap();
            if let InternalServiceResponse::DownloadFileFailure(DownloadFileFailure {
                cid: _,
                message,
                request_id: _,
            }) = download_file_response
            {
                panic!("Download File Failure: {message:?}")
            }

            // Exhaust the download request. The reception ticks must carry
            // the DownloadFile request_id (SERVER_SCOPE registration in
            // kernel/revfs_correlation.rs) — the browser settles its download
            // on the ReceptionComplete tick correlated by that id; with the
            // old TCP-uuid fallback every completed download reported failure.
            exhaust_revfs_ticks_asserting_request_id(
                from_service,
                download_request_id,
                Some(file_to_send.clone()),
            )
            .await;

            // Delete file from REVFS
            let file_delete_command = InternalServiceRequest::DeleteVirtualFile {
                virtual_directory: virtual_path.clone(),
                cid: *cid,
                peer_cid: None,
                request_id: Uuid::new_v4(),
            };
            to_service.send(file_delete_command).unwrap();
            info!(target: "citadel","DeleteVirtualFile Request sent to server");

            let file_delete_command = from_service.recv().await.unwrap();

            match file_delete_command {
                InternalServiceResponse::DeleteVirtualFileSuccess(DeleteVirtualFileSuccess {
                    cid: response_cid,
                    request_id: _,
                }) => {
                    assert_eq!(*cid, response_cid);
                    info!(target: "citadel","CID Comparison Yielded Success");
                }
                _ => {
                    info!(target = "citadel", "{:?}", file_delete_command);
                    panic!("Didn't get the REVFS DeleteVirtualFileSuccess");
                }
            }
            info!(target: "citadel","{file_delete_command:?}");

            Ok(())
        } else {
            panic!("Service Spawn Error");
        }
    }

    #[tokio::test]
    async fn test_internal_service_peer_revfs() -> Result<(), Box<dyn Error>> {
        crate::common::setup_log();
        // internal service for peer A
        let bind_address_internal_service_a: SocketAddr =
            format!("127.0.0.1:{}", get_free_port()).parse().unwrap();
        // internal service for peer B
        let bind_address_internal_service_b: SocketAddr =
            format!("127.0.0.1:{}", get_free_port()).parse().unwrap();

        let mut peer_return_handle_vec =
            register_and_connect_to_server_then_peers::<StackedRatchet>(
                vec![
                    bind_address_internal_service_a,
                    bind_address_internal_service_b,
                ],
                None,
                None,
            )
            .await?;

        let (peer_one, peer_two) = peer_return_handle_vec.as_mut_slice().split_at_mut(1_usize);
        let (to_service_a, from_service_a, cid_a) = peer_one.get_mut(0_usize).unwrap();
        let (_to_service_b, from_service_b, cid_b) = peer_two.get_mut(0_usize).unwrap();

        // Push file to REVFS on peer
        let file_to_send = PathBuf::from("../resources/test.txt");
        let virtual_path = PathBuf::from("/vfs/test.txt");
        let push_request_id = Uuid::new_v4();
        let send_file_to_service_b_payload = InternalServiceRequest::SendFile {
            request_id: push_request_id,
            source: FileSource::Path(file_to_send.clone()),
            cid: *cid_a,
            transfer_type: TransferType::RemoteEncryptedVirtualFilesystem {
                virtual_path: virtual_path.clone(),
                security_level: Default::default(),
            },
            peer_cid: Some(*cid_b),
            chunk_size: None,
        };
        to_service_a.send(send_file_to_service_b_payload).unwrap();
        let deserialized_service_a_payload_response = from_service_a.recv().await.unwrap();
        info!(target: "citadel","{deserialized_service_a_payload_response:?}");

        let InternalServiceResponse::SendFileRequestSuccess(SendFileRequestSuccess { .. }) =
            &deserialized_service_a_payload_response
        else {
            panic!("File Transfer Request failed: {deserialized_service_a_payload_response:?}");
        };

        // B's internal service AUTO-ACCEPTS REVFS pushes (see
        // responses/object_transfer_handle.rs): a REVFS storage write is an
        // internal protocol mechanism, not a user-facing transfer offer, and
        // no client ever answered the old accept prompt — so the bytes were
        // never streamed while the uploader's tree already listed the file.
        // B therefore receives no FileTransferRequestNotification and sends
        // no RespondFileTransfer; its first events are the reception ticks.
        exhaust_stream_to_file_completion(file_to_send.clone(), from_service_b).await;

        // A's Sender ticks complete the upload and must carry the SendFile
        // request_id (kernel/revfs_correlation.rs) — the browser resolves its
        // upload on the TransferComplete tick correlated by that id.
        exhaust_revfs_ticks_asserting_request_id(from_service_a, push_request_id, None).await;

        // Download P2P REVFS file - without delete on pull
        let download_request_id = Uuid::new_v4();
        let download_file_command = InternalServiceRequest::DownloadFile {
            virtual_directory: virtual_path.clone(),
            security_level: None,
            delete_on_pull: false,
            cid: *cid_a,
            peer_cid: Some(*cid_b),
            request_id: download_request_id,
        };
        to_service_a.send(download_file_command).unwrap();
        let download_file_response = from_service_a.recv().await.unwrap();
        match download_file_response {
            InternalServiceResponse::DownloadFileSuccess(DownloadFileSuccess {
                cid: response_cid,
                request_id: _,
            }) => {
                assert_eq!(*cid_a, response_cid);
            }
            _ => {
                panic!("Didn't get the REVFS DownloadFileSuccess - instead got {download_file_response:?}");
            }
        }

        // B is the byte-holder answering the pull; it issued no request, so
        // its ticks keep the legacy uuid fallback. A's reception ticks must
        // carry the DownloadFile request_id (kernel/revfs_correlation.rs).
        exhaust_stream_to_file_completion(file_to_send.clone(), from_service_b).await;
        exhaust_revfs_ticks_asserting_request_id(
            from_service_a,
            download_request_id,
            Some(file_to_send.clone()),
        )
        .await;

        // Delete file on Peer REVFS
        let delete_file_command = InternalServiceRequest::DeleteVirtualFile {
            virtual_directory: virtual_path,
            cid: *cid_a,
            peer_cid: Some(*cid_b),
            request_id: Uuid::new_v4(),
        };
        to_service_a.send(delete_file_command).unwrap();
        let delete_file_response = from_service_a.recv().await.unwrap();
        match delete_file_response {
            InternalServiceResponse::DeleteVirtualFileSuccess(DeleteVirtualFileSuccess {
                cid: response_cid,
                request_id: _,
            }) => {
                assert_eq!(*cid_a, response_cid);
            }
            _ => {
                panic!("Didn't get the REVFS DeleteVirtualFileSuccess - instead got {delete_file_response:?}");
            }
        }
        info!(target: "citadel","{delete_file_response:?}");

        Ok(())
    }

    /// Happy path for `FileSource::ByteContents`: a browser-style upload
    /// that materialises inline bytes into a temp file before handing the
    /// path to the SDK.
    ///
    /// This exercises the entire ByteContents code path - size guard, name
    /// sanitisation, `spawn_blocking` write, scheduled cleanup, and the
    /// SDK's subsequent `File::open` of the temp path - and uses the
    /// existing `exhaust_stream_to_file_completion` helper to confirm the
    /// streamed bytes match the original. If the cleanup race that
    /// previously lived in this handler ever returned, this test would
    /// flake (the SDK would observe ENOENT on open instead of completing).
    /// Sends one P2P message and asserts the peer receives exactly it.
    ///
    /// `context` names what a failure means, because the two call sites below
    /// fail for opposite reasons and a shared "message not received" would say
    /// neither.
    #[allow(clippy::too_many_arguments)]
    async fn send_and_expect_message(
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
    async fn next_ignoring_transfer_noise(
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

    /// A peer-to-peer message must still arrive AFTER a file transfer.
    ///
    /// CI run 33347976897 (`test:file-manager`): Alice's REVFS `PlaceFile`
    /// message, sent 20ms after a `SendFile` to the same peer, never reached
    /// Bob. 98 retransmits over ~110s, no delivery, while Bob->Alice kept
    /// working the whole time. Alice's internal service logged the complete
    /// chain for every one of them -- peer present in `conn.peers`, sink
    /// cloned, `sink.send() SUCCEEDED` -- so the loss is below this layer.
    ///
    /// The suite had file-transfer tests and message tests and nothing that
    /// did both in sequence, which is exactly the seam the defect lives in.
    /// This test is that missing case: message, transfer, message.
    ///
    /// The FIRST message is not ceremony. Without it a failure here cannot
    /// distinguish "the transfer broke messaging" from "messaging between
    /// these two never worked in this fixture".
    ///
    /// Narrowed by experiment: swapping this REVFS push for a plain
    /// `TransferType::FileTransfer` with an explicit accept fails identically,
    /// so the cause is NOT the REVFS auto-accept path. Any peer object
    /// transfer does it.
    /// Ignored because it FAILS, and the defect is upstream of this repo.
    ///
    /// Confirmed against `citadel_sdk` at `a28a3c7`, which is `master` HEAD --
    /// there is no newer SDK to move to. Remove this attribute when the SDK
    /// fix lands; the test is written to pass, not to be adjusted. Tracked as
    /// #57 (high) in citadel-workspaces/docs/PRODUCTION-READINESS.md.
    ///
    /// It is `#[ignore]` rather than deleted because it is the only
    /// reproduction of #57 that runs in a minute instead of a twelve-minute
    /// browser suite, and rather than left failing because a red that cannot
    /// go green until a third-party dependency changes teaches the suite to be
    /// ignored.
    #[ignore = "reproduces #57: a peer file transfer kills P2P messaging; fails in citadel_sdk a28a3c7 (master HEAD)"]
    #[tokio::test]
    async fn test_a_peer_message_after_a_file_transfer_still_arrives() -> Result<(), Box<dyn Error>>
    {
        crate::common::setup_log();
        let bind_address_internal_service_a: SocketAddr =
            format!("127.0.0.1:{}", get_free_port()).parse().unwrap();
        let bind_address_internal_service_b: SocketAddr =
            format!("127.0.0.1:{}", get_free_port()).parse().unwrap();

        let mut peer_return_handle_vec =
            register_and_connect_to_server_then_peers::<StackedRatchet>(
                vec![
                    bind_address_internal_service_a,
                    bind_address_internal_service_b,
                ],
                None,
                None,
            )
            .await?;

        let (peer_one, peer_two) = peer_return_handle_vec.as_mut_slice().split_at_mut(1_usize);
        let (to_service_a, from_service_a, cid_a) = peer_one.get_mut(0_usize).unwrap();
        let (_to_service_b, from_service_b, cid_b) = peer_two.get_mut(0_usize).unwrap();

        // Baseline: messaging works between these two before any transfer.
        send_and_expect_message(
            to_service_a,
            from_service_a,
            from_service_b,
            *cid_a,
            *cid_b,
            b"before the transfer",
            "messaging was already broken before any file transfer, so this \
             fixture cannot say anything about the transfer",
        )
        .await;

        // The REVFS push, exactly as revfs-io-network.ts issues it.
        let file_to_send = PathBuf::from("../resources/test.txt");
        let virtual_path = PathBuf::from("/vfs/after-transfer.txt");
        let push_request_id = Uuid::new_v4();
        to_service_a
            .send(InternalServiceRequest::SendFile {
                request_id: push_request_id,
                source: FileSource::Path(file_to_send.clone()),
                cid: *cid_a,
                transfer_type: TransferType::RemoteEncryptedVirtualFilesystem {
                    virtual_path: virtual_path.clone(),
                    security_level: Default::default(),
                },
                peer_cid: Some(*cid_b),
                chunk_size: None,
            })
            .unwrap();
        let push_response = from_service_a.recv().await.unwrap();
        let InternalServiceResponse::SendFileRequestSuccess(SendFileRequestSuccess { .. }) =
            &push_response
        else {
            panic!("File Transfer Request failed: {push_response:?}");
        };

        // B auto-accepts a REVFS push; its first events are reception ticks.
        exhaust_stream_to_file_completion(file_to_send.clone(), from_service_b).await;
        exhaust_revfs_ticks_asserting_request_id(from_service_a, push_request_id, None).await;

        // The message the browser sends next: the tree op naming the file.
        send_and_expect_message(
            to_service_a,
            from_service_a,
            from_service_b,
            *cid_a,
            *cid_b,
            b"after the transfer",
            "a file transfer silently killed messaging to that peer: the send \
             reports success and the peer never receives it",
        )
        .await;

        Ok(())
    }

    #[tokio::test]
    async fn test_internal_service_byte_contents_file_transfer_c2s() -> Result<(), Box<dyn Error>> {
        // Surface panics from the spawned server/service tasks (which otherwise
        // abort only their own task) as a hard process exit so this test fails
        // loudly instead of hanging. The hook is process-global, so restore the
        // *original* hook on scope exit to keep it from terminating — or merely
        // stripping the custom handler of — a later test that shares this binary
        // under `cargo test` (nextest already isolates each test per process).
        // The original hook is shared via `Arc`: the panic closure clones it to
        // still print the default panic message before `exit(1)` (preserving the
        // fail-fast intent), and the guard clones it to reinstall it on drop.
        type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>;
        let orig_hook: std::sync::Arc<PanicHook> = std::sync::Arc::new(take_hook());
        let hook_for_panic = orig_hook.clone();
        set_hook(Box::new(move |panic_info| {
            hook_for_panic(panic_info);
            exit(1);
        }));
        struct RestorePanicHookOnDrop(std::sync::Arc<PanicHook>);
        impl Drop for RestorePanicHookOnDrop {
            fn drop(&mut self) {
                let orig = self.0.clone();
                set_hook(Box::new(move |info| orig(info)));
            }
        }
        let _restore_hook = RestorePanicHookOnDrop(orig_hook);

        crate::common::setup_log();
        let bind_address_internal_service: SocketAddr =
            format!("127.0.0.1:{}", get_free_port()).parse().unwrap();

        let server_success = &Arc::new(AtomicBool::new(false));
        let (server, server_bind_address) =
            server_info_file_transfer::<StackedRatchet>(server_success.clone());
        tokio::task::spawn(server);

        let internal_service_kernel =
            CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(bind_address_internal_service)
                .await?;
        let internal_service = NodeBuilder::default()
            .with_backend(BackendType::Filesystem("filesystem".into()))
            .with_node_type(NodeType::Peer)
            .with_insecure_skip_cert_verification()
            .build(internal_service_kernel)?;

        tokio::task::spawn(internal_service);
        tokio::time::sleep(Duration::from_millis(2000)).await;

        let to_spawn = vec![RegisterAndConnectItems {
            internal_service_addr: bind_address_internal_service,
            server_addr: server_bind_address,
            full_name: "Browser User",
            username: "browser.user",
            password: "secret",
            pre_shared_key: None::<PreSharedKey>,
        }];
        let returned_service_info = register_and_connect_to_server(to_spawn).await;
        let mut service_vec = returned_service_info.unwrap();
        if let Some((to_service, from_service, cid)) = service_vec.get_mut(0_usize) {
            // Read the same fixture as the path-based test, but transfer
            // it via ByteContents so we exercise the temp-file path.
            let cmp_path = PathBuf::from("../resources/test.txt");
            let bytes = std::fs::read(&cmp_path)?;

            let file_transfer_command = InternalServiceRequest::SendFile {
                request_id: Uuid::new_v4(),
                source: FileSource::ByteContents {
                    file_name: "test.txt".to_string(),
                    data: bytes,
                },
                cid: *cid,
                transfer_type: TransferType::FileTransfer,
                peer_cid: None,
                chunk_size: None,
            };
            to_service.send(file_transfer_command).unwrap();
            exhaust_stream_to_file_completion(cmp_path, from_service).await;

            Ok(())
        } else {
            panic!("Service Spawn Error")
        }
    }

    /// Confirms that an oversized `ByteContents` payload is rejected with
    /// `SendFileRequestFailure` *before* any temp file is created. Uses a
    /// payload one byte larger than the handler's 16 MiB cap. The size
    /// fits comfortably within the TCP `LengthDelimitedCodec`'s 64 MiB
    /// frame limit (TCP encodes via bincode2, ~1:1, so 16 MiB stays
    /// under 64 MiB without any JSON-style expansion), so the request
    /// actually reaches the handler and exercises the in-handler size
    /// guard rather than being rejected at the framing layer.
    #[tokio::test]
    async fn test_internal_service_byte_contents_size_limit_rejected() -> Result<(), Box<dyn Error>>
    {
        crate::common::setup_log();
        let bind_address_internal_service: SocketAddr =
            format!("127.0.0.1:{}", get_free_port()).parse().unwrap();

        let server_success = &Arc::new(AtomicBool::new(false));
        let (server, server_bind_address) =
            server_info_file_transfer::<StackedRatchet>(server_success.clone());
        tokio::task::spawn(server);

        let internal_service_kernel =
            CitadelWorkspaceService::<_, StackedRatchet>::new_tcp(bind_address_internal_service)
                .await?;
        let internal_service = NodeBuilder::default()
            .with_backend(BackendType::Filesystem("filesystem".into()))
            .with_node_type(NodeType::Peer)
            .with_insecure_skip_cert_verification()
            .build(internal_service_kernel)?;

        tokio::task::spawn(internal_service);
        tokio::time::sleep(Duration::from_millis(2000)).await;

        let to_spawn = vec![RegisterAndConnectItems {
            internal_service_addr: bind_address_internal_service,
            server_addr: server_bind_address,
            full_name: "Oversize User",
            username: "oversize.user",
            password: "secret",
            pre_shared_key: None::<PreSharedKey>,
        }];
        let returned_service_info = register_and_connect_to_server(to_spawn).await;
        let mut service_vec = returned_service_info.unwrap();
        if let Some((to_service, from_service, cid)) = service_vec.get_mut(0_usize) {
            // 16 MiB + 1 byte: just past the handler's MAX_BYTE_CONTENTS_BYTES
            // cap. TCP encodes via bincode2 (~1:1), so the raw 16 MiB stays
            // well under the 64 MiB framing limit and reaches the handler.
            let oversize: Vec<u8> = vec![0u8; 16 * 1024 * 1024 + 1];

            let file_transfer_command = InternalServiceRequest::SendFile {
                request_id: Uuid::new_v4(),
                source: FileSource::ByteContents {
                    file_name: "huge.bin".to_string(),
                    data: oversize,
                },
                cid: *cid,
                transfer_type: TransferType::FileTransfer,
                peer_cid: None,
                chunk_size: None,
            };
            to_service.send(file_transfer_command).unwrap();

            // Must come back as a SendFileRequestFailure with a message
            // mentioning the size cap. We don't tightly couple to the
            // exact wording - just that the handler refused.
            let response = from_service.recv().await.unwrap();
            match response {
                InternalServiceResponse::SendFileRequestFailure(SendFileRequestFailure {
                    message,
                    ..
                }) => {
                    assert!(
                        message.contains("exceeds") && message.contains("maximum"),
                        "expected an oversize-rejection message, got: {message:?}"
                    );
                }
                other => panic!("expected SendFileRequestFailure, got {other:?}"),
            }

            Ok(())
        } else {
            panic!("Service Spawn Error")
        }
    }

    /// Acceptor-side P2P upload + download via REVFS. Pins the
    /// `VirtualTargetType::LocalGroupPeer { session_cid, peer_cid }`
    /// fix in `upload.rs`/`download.rs`: the previous implementations
    /// dereferenced `peer_conn.remote.user()`, which is `None` on the
    /// side that ACCEPTED the original `PeerConnect` (i.e. the
    /// non-initiator) — so any file send/download initiated by the
    /// acceptor failed with "Peer connection missing remote". The
    /// existing `test_internal_service_peer_revfs` only exercises the
    /// initiator side (peer A drives every transfer) and would have
    /// kept passing even with the acceptor bug present. This test
    /// flips the direction: peer B (acceptor) uploads to A's REVFS
    /// and then downloads it back, hitting both fixes.
    #[tokio::test]
    async fn test_internal_service_peer_revfs_acceptor_initiates() -> Result<(), Box<dyn Error>> {
        crate::common::setup_log();
        let bind_address_internal_service_a: SocketAddr =
            format!("127.0.0.1:{}", get_free_port()).parse().unwrap();
        let bind_address_internal_service_b: SocketAddr =
            format!("127.0.0.1:{}", get_free_port()).parse().unwrap();

        let mut peer_return_handle_vec =
            register_and_connect_to_server_then_peers::<StackedRatchet>(
                vec![
                    bind_address_internal_service_a,
                    bind_address_internal_service_b,
                ],
                None,
                None,
            )
            .await?;

        // After `register_and_connect_to_server_then_peers`, peer A is
        // the P2P initiator (it sent the first `PeerConnect`); peer B
        // is the acceptor. All requests below are issued from B → A
        // so the `peer_conn.remote == None` branch is the one being
        // exercised.
        let (peer_one, peer_two) = peer_return_handle_vec.as_mut_slice().split_at_mut(1_usize);
        let (_to_service_a, from_service_a, cid_a) = peer_one.get_mut(0_usize).unwrap();
        let (to_service_b, from_service_b, cid_b) = peer_two.get_mut(0_usize).unwrap();

        let file_to_send = PathBuf::from("../resources/test.txt");
        // Match the source basename so `exhaust_stream_to_file_completion`'s
        // `vfm.name == cmp_file_name` assertion lines up — the REVFS
        // `vfm.name` is the virtual path's basename, not the source path's.
        let virtual_path = PathBuf::from("/vfs/test.txt");

        // B (acceptor) uploads to A's REVFS — exercises the
        // `upload.rs` `LocalGroupPeer` fix.
        let push_request_id = Uuid::new_v4();
        let send_from_b = InternalServiceRequest::SendFile {
            request_id: push_request_id,
            source: FileSource::Path(file_to_send.clone()),
            cid: *cid_b,
            transfer_type: TransferType::RemoteEncryptedVirtualFilesystem {
                virtual_path: virtual_path.clone(),
                security_level: Default::default(),
            },
            peer_cid: Some(*cid_a),
            chunk_size: None,
        };
        to_service_b.send(send_from_b).unwrap();

        let send_resp = from_service_b.recv().await.unwrap();
        let InternalServiceResponse::SendFileRequestSuccess(SendFileRequestSuccess { .. }) =
            send_resp
        else {
            panic!("Acceptor-side SendFile failed: {send_resp:?}");
        };

        // A's internal service auto-accepts the REVFS push (see
        // responses/object_transfer_handle.rs and the note in
        // test_internal_service_peer_revfs) — no notification, no
        // RespondFileTransfer; A's first events are the reception ticks.
        exhaust_stream_to_file_completion(file_to_send.clone(), from_service_a).await;
        exhaust_revfs_ticks_asserting_request_id(from_service_b, push_request_id, None).await;

        // B (acceptor) pulls the same file back from A's REVFS —
        // exercises the `download.rs` `LocalGroupPeer` fix.
        let download_request_id = Uuid::new_v4();
        let download_from_b = InternalServiceRequest::DownloadFile {
            virtual_directory: virtual_path.clone(),
            security_level: None,
            delete_on_pull: false,
            cid: *cid_b,
            peer_cid: Some(*cid_a),
            request_id: download_request_id,
        };
        to_service_b.send(download_from_b).unwrap();

        let download_resp = from_service_b.recv().await.unwrap();
        match download_resp {
            InternalServiceResponse::DownloadFileSuccess(DownloadFileSuccess { cid, .. }) => {
                assert_eq!(cid, *cid_b);
            }
            InternalServiceResponse::DownloadFileFailure(DownloadFileFailure {
                message, ..
            }) => panic!("Acceptor-side download rejected with: {message}"),
            other => panic!("Unexpected response to acceptor-side DownloadFile: {other:?}"),
        }

        // A answers the pull with the stored bytes; B's reception ticks must
        // carry the DownloadFile request_id (kernel/revfs_correlation.rs).
        exhaust_stream_to_file_completion(file_to_send.clone(), from_service_a).await;
        exhaust_revfs_ticks_asserting_request_id(
            from_service_b,
            download_request_id,
            Some(file_to_send.clone()),
        )
        .await;

        Ok(())
    }
}

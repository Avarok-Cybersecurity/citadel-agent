use crate::kernel::conversations::kv::HostKv;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::{spawn_tick_updater, CitadelWorkspaceService};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    FileTransferStatusNotification, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::prelude::Ratchet;
use std::sync::Arc;
use uuid::Uuid;

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    // TODO: Factor-in download location?
    let InternalServiceRequest::RespondFileTransfer {
        cid,
        peer_cid,
        object_id,
        accept,
        download_location: _,
        request_id,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };
    let mut server_connection_map = this.server_connection_map.write();

    let response = if let Some(connection) = server_connection_map.get_mut(&cid) {
        if let Some(mut handler) = connection.take_file_transfer_handle(peer_cid, object_id) {
            if let Some(mut owned_handler) = handler.take() {
                let result = if accept {
                    let accept_result = owned_handler.accept();
                    // The saved file is this session's to read into the page,
                    // which is how a click on it downloads the original.
                    let on_received = crate::kernel::received_reads::on_received(
                        this.server_connection_map.clone(),
                        Arc::new(HostKv::account(this.host_io(), cid)),
                        cid,
                        peer_cid,
                    );
                    spawn_tick_updater(
                        owned_handler,
                        cid,
                        Some(peer_cid),
                        &mut server_connection_map,
                        this.tx_to_localhost_clients.clone(),
                        Some(request_id),
                        crate::kernel::tick_updater::StreamHooks {
                            on_pulled: Some(on_received),
                            on_end: None,
                        },
                    );

                    accept_result
                } else {
                    owned_handler.decline()
                };
                match result {
                    Ok(_) => InternalServiceResponse::FileTransferStatusNotification(
                        FileTransferStatusNotification {
                            cid,
                            object_id,
                            success: true,
                            response: accept,
                            message: None,
                            request_id: Some(request_id),
                        },
                    ),

                    Err(err) => InternalServiceResponse::FileTransferStatusNotification(
                        FileTransferStatusNotification {
                            cid,
                            object_id,
                            success: false,
                            response: accept,
                            message: Option::from(err.into_string()),
                            request_id: Some(request_id),
                        },
                    ),
                }
            } else {
                InternalServiceResponse::FileTransferStatusNotification(
                    FileTransferStatusNotification {
                        cid,
                        object_id,
                        success: false,
                        response: accept,
                        message: Some("File transfer handlernot found".to_string()),
                        request_id: Some(request_id),
                    },
                )
            }
        } else {
            InternalServiceResponse::FileTransferStatusNotification(
                FileTransferStatusNotification {
                    cid,
                    object_id,
                    success: false,
                    response: accept,
                    message: Some("File transfer not found".to_string()),
                    request_id: Some(request_id),
                },
            )
        }
    } else {
        InternalServiceResponse::FileTransferStatusNotification(FileTransferStatusNotification {
            cid,
            object_id,
            success: false,
            response: accept,
            message: Some("Connection not found".to_string()),
            request_id: Some(request_id),
        })
    };

    Some(HandledRequestResult { response, uuid })
}

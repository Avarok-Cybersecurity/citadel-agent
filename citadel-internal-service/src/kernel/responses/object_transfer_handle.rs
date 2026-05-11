use crate::kernel::{spawn_tick_updater, CitadelWorkspaceService};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{FileTransferRequestNotification, InternalServiceResponse};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{
    NetworkError, ObjectTransferHandle, ObjectTransferOrientation, Ratchet,
};
use std::sync::atomic::Ordering;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    object_transfer_handle: ObjectTransferHandle,
) -> Result<(), NetworkError> {
    let metadata = object_transfer_handle.handle.metadata.clone();
    let object_id = metadata.object_id;
    let implicated_cid = object_transfer_handle.session_cid;
    let peer_cid = if object_transfer_handle.handle.receiver != implicated_cid {
        object_transfer_handle.handle.receiver
    } else {
        object_transfer_handle.handle.source
    };
    let object_transfer_handler = object_transfer_handle.handle;

    citadel_sdk::logging::info!(target: "citadel", "Orientation: {:?}", object_transfer_handler.orientation);
    citadel_sdk::logging::info!(target: "citadel", "ObjectTransferHandle has implicated_cid: {implicated_cid:?} and peer_cid {peer_cid:?}");

    // When we receive a handle, there are two possibilities:
    // A: We are the sender of the file transfer, in which case we can assume the adjacent node
    // already accepted the file transfer request, and therefore we can spawn a task to forward
    // the ticks immediately
    //
    // B: We are the receiver of the file transfer. We need to wait for the TCP client to accept
    // the request, thus, we need to store it. UNLESS, this is an revfs pull, in which case we
    // allow the transfer to proceed immediately since the protocol auto accepts these requests
    if let ObjectTransferOrientation::Receiver { is_revfs_pull } =
        object_transfer_handler.orientation
    {
        info!(target: "citadel", "Receiver Obtained ObjectTransferHandler");

        let mut server_connection_map = this.server_connection_map.write();
        if let Some(connection) = server_connection_map.get_mut(&implicated_cid) {
            let uuid = connection
                .associated_localhost_connection
                .load(Ordering::Relaxed);

            if is_revfs_pull {
                spawn_tick_updater(
                    object_transfer_handler,
                    implicated_cid,
                    Some(peer_cid),
                    &mut server_connection_map,
                    this.tx_to_localhost_clients.clone(),
                    None,
                );
            } else {
                // Send an update to the TCP client that way they can choose to accept or reject the transfer
                let response = InternalServiceResponse::FileTransferRequestNotification(
                    FileTransferRequestNotification {
                        cid: implicated_cid,
                        peer_cid,
                        metadata,
                        request_id: None,
                    },
                );

                connection.add_object_transfer_handler(
                    peer_cid,
                    object_id,
                    Some(object_transfer_handler),
                );

                drop(server_connection_map);

                // Same robustness story as peer_channel_created.rs: a stale
                // `associated_localhost_connection` UUID (post ClaimSession /
                // tab-reload) can leave a not-yet-GC'd TCP entry whose
                // `send()` returns Ok but lands on a closed receiver. Check
                // `is_closed()` AND broadcast to other live connections so
                // the FileTransferRequestNotification reaches whichever tab
                // currently owns the session. Frontend filters duplicates
                // by `cid`. Without this, a fresh user that reloads mid-
                // accept-flow never sees the file prompt.
                let tcp_map = this.tx_to_localhost_clients.read();
                let target_alive = tcp_map.get(&uuid)
                    .map(|s| !s.is_closed())
                    .unwrap_or(false);
                let mut sent_count = 0;
                if target_alive {
                    if let Some(entry) = tcp_map.get(&uuid) {
                        if entry.send(response.clone()).is_ok() {
                            sent_count += 1;
                        }
                    }
                }
                for (other_uuid, sender) in tcp_map.iter() {
                    if *other_uuid == uuid { continue; }
                    if sender.is_closed() { continue; }
                    if sender.send(response.clone()).is_ok() {
                        sent_count += 1;
                    }
                }
                if sent_count == 0 {
                    warn!(target: "citadel", "[ObjectTransferHandle] No live TCP connections; FileTransferRequestNotification dropped for cid={implicated_cid}, peer_cid={peer_cid}");
                } else {
                    info!(target: "citadel", "[ObjectTransferHandle] Delivered FileTransferRequestNotification to {sent_count} connection(s)");
                }
                drop(tcp_map);
            }
        }
    } else {
        // Sender - Must spawn a task to relay status updates to TCP client. When receiving this handle,
        // we know the opposite node agreed to the connection thus we can spawn
        let mut server_connection_map = this.server_connection_map.write();
        info!(target: "citadel", "Sender Obtained ObjectTransferHandler");
        spawn_tick_updater(
            object_transfer_handler,
            implicated_cid,
            Some(peer_cid),
            &mut server_connection_map,
            this.tx_to_localhost_clients.clone(),
            None,
        );
    }

    Ok(())
}

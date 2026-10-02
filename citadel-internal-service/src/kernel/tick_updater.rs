//! File-transfer progress, reported to every window attached to the session.

use crate::kernel::session_route::SessionRoute;
use crate::kernel::Connection;
use citadel_internal_service_types::{FileTransferTickNotification, InternalServiceResponse};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::*;
use futures::stream::StreamExt;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

pub(crate) fn spawn_tick_updater<R: Ratchet>(
    object_transfer_handler: ObjectTransferHandler,
    implicated_cid: u64,
    peer_cid: Option<u64>,
    server_connection_map: &mut HashMap<u64, Connection<R>>,
    tcp_connection_map: Arc<RwLock<HashMap<Uuid, UnboundedSender<InternalServiceResponse>>>>,
    request_id: Option<Uuid>,
) {
    let mut handle_inner = object_transfer_handler.inner;
    if let Some(connection) = server_connection_map.get_mut(&implicated_cid) {
        // The REQUEST id may be frozen -- it names the request that started the
        // transfer and does not change. The ROUTE may not: windows attach and
        // drop mid-transfer and every remaining tick has to reach whoever is
        // attached. See kernel/session_route.rs.
        let request_id = Some(
            request_id
                .unwrap_or_else(|| connection.subscribers.primary().unwrap_or_else(Uuid::nil)),
        );
        let route = SessionRoute::new(connection.subscribers.clone(), tcp_connection_map);
        let sender_status_updater = async move {
            while let Some(status) = handle_inner.next().await {
                let status_message = status.clone();
                let message = InternalServiceResponse::FileTransferTickNotification(
                    FileTransferTickNotification {
                        cid: implicated_cid,
                        peer_cid,
                        status: status_message,
                        request_id,
                    },
                );
                match route.send(message).as_slice() {
                    [] => {
                        warn!(target: "citadel", "No localhost connection owns CID {implicated_cid} - File Transfer Status Tick dropped")
                    }
                    targets => {
                        info!(target: "citadel", "File Transfer Status Tick Sent to {targets:?}: {status:?}")
                    }
                }

                // Outside the delivery result on purpose. The transfer is over
                // whether or not anybody was listening; keeping the task alive
                // because a tab happened to be closed is how these leak.
                if matches!(
                    status,
                    ObjectTransferStatus::TransferComplete
                        | ObjectTransferStatus::ReceptionComplete
                ) {
                    info!(target: "citadel", "File Transfer Completed - Ending Tick Updater");
                    break;
                }
            }
            info!(target:"citadel", "Spawned Tick Updater has ended for {implicated_cid:?}");
        };
        tokio::task::spawn(sender_status_updater);
    } else {
        info!(target: "citadel", "tick_updater: Server Connection Not Found")
    }
}

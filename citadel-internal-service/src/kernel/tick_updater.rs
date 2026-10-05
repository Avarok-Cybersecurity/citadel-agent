//! File-transfer progress, reported to every window attached to the session.

use crate::kernel::pulled_files::{PulledFileHook, PulledOutput};
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

/// What a stream's task does besides reporting ticks.
#[derive(Default)]
pub(crate) struct StreamHooks {
    /// Told the local output of a RE-VFS pull once it is complete.
    pub on_pulled: Option<PulledFileHook>,
    /// Run once the stream has ended, however it ended (completed, failed, or
    /// the SDK dropped it): e.g. removing the browser payload a send read.
    pub on_end: Option<Box<dyn FnOnce() + Send>>,
}

pub(crate) fn spawn_tick_updater<R: Ratchet>(
    object_transfer_handler: ObjectTransferHandler,
    implicated_cid: u64,
    peer_cid: Option<u64>,
    server_connection_map: &mut HashMap<u64, Connection<R>>,
    tcp_connection_map: Arc<RwLock<HashMap<Uuid, UnboundedSender<InternalServiceResponse>>>>,
    request_id: Option<Uuid>,
    hooks: StreamHooks,
) {
    let StreamHooks { on_pulled, on_end } = hooks;
    let mut handle_inner = object_transfer_handler.inner;
    if let Some(connection) = server_connection_map.get_mut(&implicated_cid) {
        // The REQUEST id is frozen -- it names the request that started the
        // transfer, or is None for a stream nobody here requested. It used to
        // fall back to the session's TCP uuid, which every such stream shared:
        // the browser filed that uuid as "not a chat transfer" on the first
        // RE-VFS reception and then dropped every chat send's ticks with it.
        // The ROUTE may change: windows attach and drop mid-transfer and every
        // remaining tick has to reach whoever is attached. See
        // kernel/session_route.rs.
        let route = SessionRoute::new(connection.subscribers.clone(), tcp_connection_map);
        let sender_status_updater = async move {
            let mut on_pulled = on_pulled;
            let mut pulled = PulledOutput::default();
            while let Some(status) = handle_inner.next().await {
                if on_pulled.is_some() {
                    if let Some(info) = pulled.observe(&status, std::time::Instant::now()) {
                        if let Some(hook) = on_pulled.take() {
                            hook(info);
                        }
                    }
                }
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
            if let Some(on_end) = on_end {
                on_end();
            }
        };
        tokio::task::spawn(sender_status_updater);
    } else {
        info!(target: "citadel", "tick_updater: Server Connection Not Found");
        if let Some(on_end) = on_end {
            on_end();
        }
    }
}

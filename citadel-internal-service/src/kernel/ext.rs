use crate::kernel::{send_to_kernel, sink_send_payload, Connection};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServicePayload, InternalServiceResponse, ServiceConnectionAccepted,
};
use citadel_sdk::logging::{debug, error, info, warn};
use citadel_sdk::prelude::Ratchet;
use futures::StreamExt;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

use citadel_internal_service_types::InternalServiceRequest;

pub trait IOInterfaceExt: IOInterface {
    #[allow(clippy::too_many_arguments)]
    fn spawn_connection_handler<R: Ratchet>(
        &mut self,
        mut sink: Self::Sink,
        mut stream: Self::Stream,
        to_kernel: UnboundedSender<(InternalServiceRequest, Uuid)>,
        mut from_kernel: UnboundedReceiver<InternalServiceResponse>,
        conn_id: Uuid,
        tcp_connection_map: Arc<RwLock<HashMap<Uuid, UnboundedSender<InternalServiceResponse>>>>,
        server_connection_map: Arc<RwLock<HashMap<u64, Connection<R>>>>,
        orphan_sessions: Arc<RwLock<HashMap<Uuid, bool>>>,
    ) {
        tokio::task::spawn(async move {
            let write_task = async {
                let response =
                    InternalServiceResponse::ServiceConnectionAccepted(ServiceConnectionAccepted {
                        cid: 0,
                        request_id: Some(conn_id),
                    });

                if let Err(err) = sink_send_payload::<Self>(response, &mut sink).await {
                    error!(target: "citadel", "Failed to send to client: {err:?}");
                    return;
                }

                while let Some(kernel_response) = from_kernel.recv().await {
                    debug!(target: "citadel", "Sending kernel response to client: {:?}", kernel_response);
                    if let Err(err) = sink_send_payload::<Self>(kernel_response, &mut sink).await {
                        error!(target: "citadel", "Failed to send to client: {err:?}");
                        return;
                    }
                }
            };

            let read_task = async {
                while let Some(message) = stream.next().await {
                    match message {
                        Ok(message) => {
                            if let InternalServicePayload::Request(request) = message {
                                if let Err(err) = send_to_kernel(request, &to_kernel, conn_id) {
                                    error!(target: "citadel", "Failed to send to kernel: {:?}", err);
                                    break;
                                }
                            }
                        }
                        Err(_) => {
                            warn!(target: "citadel", "Bad message from client");
                        }
                    }
                }
                debug!(target: "citadel", "Disconnected connection {conn_id:?}");
            };

            tokio::select! {
                res0 = write_task => res0,
                res1 = read_task => res1,
            }

            tcp_connection_map.write().remove(&conn_id);

            // ALWAYS preserve sessions when TCP drops.
            //
            // Sessions should persist across page navigations and reconnections.
            // This is the default behavior for modern web apps where users can:
            // - Navigate between pages
            // - Refresh the page
            // - Have multiple tabs open
            //
            // Sessions are only explicitly cleaned up via:
            // 1. Disconnect request (user-initiated logout)
            // 2. Deregister request (account deletion)
            // 3. GetSessions reconciliation (sync with SDK state)
            //
            // The orphan_sessions map is no longer used for cleanup decisions.
            // We preserve ALL sessions regardless of orphan mode setting.

            let (preserved_session_count, all_sessions, preserved_sessions_info) = {
                let lock = server_connection_map.read();
                let all: Vec<(u64, String)> = lock.iter()
                    .map(|(cid, conn)| (*cid, conn.username.clone()))
                    .collect();
                let preserved: Vec<(u64, String)> = lock.iter()
                    .filter(|(_, conn)| {
                        conn.associated_localhost_connection.load(Ordering::Relaxed) == conn_id
                    })
                    .map(|(cid, conn)| (*cid, conn.username.clone()))
                    .collect();
                (preserved.len(), all, preserved)
            };

            info!(target: "citadel", "[TCP_DISCONNECT] Connection {conn_id:?} closed. Preserving all sessions.");
            info!(target: "citadel", "[TCP_DISCONNECT] Total sessions in map: {:?}", all_sessions);
            info!(target: "citadel", "[TCP_DISCONNECT] Sessions associated with THIS connection ({conn_id:?}): {:?}", preserved_sessions_info);
            info!(target: "citadel", "[TCP_DISCONNECT] Preserved {} sessions for reconnection", preserved_session_count);

            // Clean up the orphan_sessions entry if it exists (no longer used for decisions)
            orphan_sessions.write().remove(&conn_id);
        });
    }
}

impl<T: IOInterface> IOInterfaceExt for T {}

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

            // Check if this connection is in orphan mode
            let is_orphan = orphan_sessions
                .read()
                .get(&conn_id)
                .copied()
                .unwrap_or(false);

            if is_orphan {
                info!(target: "citadel", "[ORPHAN_DEBUG] Connection {conn_id:?} is in orphan mode, preserving sessions and peer connections");

                // In orphan mode, we preserve EVERYTHING:
                // - Sessions remain in server_connection_map
                // - Peer connections remain intact (SDK P2P channels are still active)
                // - When the user reconnects and claims the session, they can continue using existing P2P connections
                //
                // We do NOT disconnect SDK P2P connections or clear peer state because:
                // 1. The SDK runs on the internal service, not the client browser
                // 2. P2P connections in SDK are between sessions, not TCP connections
                // 3. When client reconnects, they can resume using the same P2P channels

                let (orphaned_session_count, all_sessions, orphaned_sessions_info) = {
                    let lock = server_connection_map.read();
                    let all: Vec<(u64, String)> = lock.iter()
                        .map(|(cid, conn)| (*cid, conn.username.clone()))
                        .collect();
                    let orphaned: Vec<(u64, String)> = lock.iter()
                        .filter(|(_, conn)| {
                            conn.associated_localhost_connection.load(Ordering::Relaxed) == conn_id
                        })
                        .map(|(cid, conn)| (*cid, conn.username.clone()))
                        .collect();
                    (orphaned.len(), all, orphaned)
                };

                info!(target: "citadel", "[ORPHAN_DEBUG] Total sessions in map: {:?}", all_sessions);
                info!(target: "citadel", "[ORPHAN_DEBUG] Sessions associated with THIS connection ({conn_id:?}): {:?}", orphaned_sessions_info);
                info!(target: "citadel", "[ORPHAN_DEBUG] Preserved {} sessions with their peer connections for reconnection", orphaned_session_count);

                orphan_sessions.write().remove(&conn_id);
            } else {
                let mut server_connection_map = server_connection_map.write();
                // Remove all connections whose associated_tcp_connection is conn_id
                let count_before = server_connection_map.len();
                server_connection_map.retain(|_, v| {
                    v.associated_localhost_connection.load(Ordering::Relaxed) != conn_id
                });
                let count_after = server_connection_map.len();
                debug!(target: "citadel", "Removed {} connections from server_connection_map for {conn_id:?}. Reconnection will be required", count_before - count_after);
            }
        });
    }
}

impl<T: IOInterface> IOInterfaceExt for T {}

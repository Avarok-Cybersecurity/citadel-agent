use crate::kernel::{send_to_kernel, sink_send_payload, Connection};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServicePayload, InternalServiceRequest, InternalServiceResponse,
    ServiceConnectionAccepted,
};
use citadel_sdk::logging::{debug, error, warn};
use citadel_sdk::prelude::Ratchet;
use futures::StreamExt;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

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
            let write_task = async move {
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

            let read_task = async move {
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
                debug!(target: "citadel", "Connection {conn_id:?} is in orphan mode, preserving sessions");

                // CRITICAL FIX: Even in orphan mode, P2P channels are broken when TCP drops.
                // We must clear peer connections from both:
                // 1. The orphaned sessions (their P2P channels to others are broken)
                // 2. Other sessions that have the orphaned sessions as peers
                let mut server_connection_map = server_connection_map.write();

                // Find all CIDs belonging to the disconnected TCP connection
                let orphaned_cids: Vec<u64> = server_connection_map
                    .iter()
                    .filter(|(_, conn)| conn.associated_tcp_connection.load(Ordering::Relaxed) == conn_id)
                    .map(|(cid, _)| *cid)
                    .collect();

                debug!(target: "citadel", "Orphan disconnect: found {} sessions to process for peer cleanup: {:?}", orphaned_cids.len(), orphaned_cids);

                // Clear peer connections in both directions
                for (session_cid, conn) in server_connection_map.iter_mut() {
                    let is_orphaned_session = orphaned_cids.contains(session_cid);

                    if is_orphaned_session {
                        // This is an orphaned session - clear ALL its peers (channels are broken)
                        let peer_count = conn.peers.len();
                        if peer_count > 0 {
                            conn.peers.clear();
                            debug!(target: "citadel", "Cleared {} peers from orphaned session {}", peer_count, session_cid);
                        }
                    } else {
                        // This is an active session - remove orphaned peers from its peer list
                        for orphaned_cid in &orphaned_cids {
                            if conn.peers.remove(orphaned_cid).is_some() {
                                debug!(target: "citadel", "Removed orphaned peer {} from active session {}", orphaned_cid, session_cid);
                            }
                        }
                    }
                }

                drop(server_connection_map);
                // Don't remove sessions, just remove the orphan flag
                orphan_sessions.write().remove(&conn_id);
            } else {
                let mut server_connection_map = server_connection_map.write();
                // Remove all connections whose associated_tcp_connection is conn_id
                let count_before = server_connection_map.len();
                server_connection_map
                    .retain(|_, v| v.associated_tcp_connection.load(Ordering::Relaxed) != conn_id);
                let count_after = server_connection_map.len();
                debug!(target: "citadel", "Removed {} connections from server_connection_map for {conn_id:?}. Reconnection will be required", count_before - count_after);
            }
        });
    }
}

impl<T: IOInterface> IOInterfaceExt for T {}

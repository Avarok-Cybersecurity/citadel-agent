use crate::kernel::{send_to_kernel, sink_send_payload, Connection};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServicePayload, InternalServiceRequest, InternalServiceResponse,
    ServiceConnectionAccepted,
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
            // Clone to_kernel for use in orphan cleanup (original will be moved into read_task)
            let to_kernel_for_cleanup = to_kernel.clone();

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
                info!(target: "citadel", "Connection {conn_id:?} is in orphan mode, preserving sessions");

                // CRITICAL FIX: Even in orphan mode, P2P channels are broken when TCP drops.
                // We must:
                // 1. Notify the SDK about broken P2P connections (so it cleans up internal state)
                // 2. Clear peer connections from the internal service's Connection structs
                let server_connection_map_lock = server_connection_map.read();

                // Find all CIDs belonging to the disconnected TCP connection
                let orphaned_cids: Vec<u64> = server_connection_map_lock
                    .iter()
                    .filter(|(_, conn)| conn.associated_tcp_connection.load(Ordering::Relaxed) == conn_id)
                    .map(|(cid, _)| *cid)
                    .collect();

                info!(target: "citadel", "Orphan disconnect: found {} sessions to process for peer cleanup: {:?}", orphaned_cids.len(), orphaned_cids);

                // Collect all (active_session_cid, orphaned_peer_cid) pairs that need SDK disconnect
                // These are active sessions that have orphaned sessions as peers
                let mut sdk_disconnect_pairs: Vec<(u64, u64)> = Vec::new();
                for (session_cid, conn) in server_connection_map_lock.iter() {
                    let is_orphaned_session = orphaned_cids.contains(session_cid);
                    if !is_orphaned_session {
                        // Active session - collect orphaned peers for SDK disconnect
                        for orphaned_cid in &orphaned_cids {
                            if conn.peers.contains_key(orphaned_cid) {
                                sdk_disconnect_pairs.push((*session_cid, *orphaned_cid));
                            }
                        }
                    }
                }
                drop(server_connection_map_lock);

                // Send PeerDisconnect requests to SDK for active sessions with orphaned peers
                // This notifies the SDK to clean up its internal P2P state
                // IMPORTANT: Do NOT clear the peer from the map here - let disconnect.rs do it
                // after the SDK has been notified. If we clear it here, disconnect.rs will
                // fail its peer existence check and never notify the SDK.
                for (session_cid, peer_cid) in &sdk_disconnect_pairs {
                    info!(target: "citadel", "Sending SDK PeerDisconnect for active session {} -> orphaned peer {}", session_cid, peer_cid);
                    let disconnect_request = InternalServiceRequest::PeerDisconnect {
                        request_id: Uuid::new_v4(),
                        cid: *session_cid,
                        peer_cid: *peer_cid,
                    };
                    if let Err(err) = to_kernel_for_cleanup.send((disconnect_request, conn_id)) {
                        error!(target: "citadel", "Failed to send PeerDisconnect to kernel: {:?}", err);
                    }
                }

                // Only clear peers from ORPHANED sessions (their P2P channels are definitely broken).
                // For ACTIVE sessions, the PeerDisconnect handler (disconnect.rs) will clear the
                // peer after notifying the SDK. This ensures the SDK cleans up its internal state.
                let mut server_connection_map = server_connection_map.write();
                for (session_cid, conn) in server_connection_map.iter_mut() {
                    let is_orphaned_session = orphaned_cids.contains(session_cid);

                    if is_orphaned_session {
                        // This is an orphaned session - clear ALL its peers (channels are broken)
                        let peer_count = conn.peers.len();
                        if peer_count > 0 {
                            conn.peers.clear();
                            info!(target: "citadel", "Cleared {} peers from orphaned session {}", peer_count, session_cid);
                        }
                    }
                    // NOTE: For active sessions, we do NOT clear orphaned peers here.
                    // The PeerDisconnect handler will clear them after notifying the SDK.
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

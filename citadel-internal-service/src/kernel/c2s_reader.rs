//! Forwards what the server sends a session to whichever localhost client owns it.
//!
//! One reader per C2S channel. Connect starts one; a reconnect starts one for the new
//! channel, since the old one ends with the link it was reading.

use crate::kernel::Connection;
use citadel_internal_service_types::{InternalServiceResponse, MessageNotification};
use citadel_sdk::prelude::{PeerChannelRecvHalf, Ratchet};
use futures::StreamExt;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

type LocalhostClients = Arc<RwLock<HashMap<Uuid, UnboundedSender<InternalServiceResponse>>>>;

/// `request_id` tags every notification with the Connect that opened the session;
/// `fallback_uuid` is used when the session has left the map.
pub(crate) fn spawn<R: Ratchet>(
    server_conn_map: Arc<RwLock<HashMap<u64, Connection<R>>>>,
    localhost_clients: LocalhostClients,
    cid: u64,
    mut stream: PeerChannelRecvHalf<R>,
    request_id: Uuid,
    fallback_uuid: Uuid,
) {
    let connection_read_stream = async move {
        while let Some(message) = stream.next().await {
            let message = InternalServiceResponse::MessageNotification(MessageNotification {
                message: message.into_buffer().into(),
                cid,
                peer_cid: 0,
                request_id: Some(request_id),
            });

            // Get the current associated TCP connection for this session (may have changed via ClaimSession)
            let server_lock = server_conn_map.read();
            let current_tcp_uuid = server_lock
                .get(&cid)
                .map(|conn| {
                    conn.associated_localhost_connection
                        .load(std::sync::atomic::Ordering::Relaxed)
                })
                .unwrap_or(fallback_uuid);
            drop(server_lock);

            let lock = localhost_clients.read();
            match lock.get(&current_tcp_uuid) {
                Some(entry) => {
                    if let Err(err) = entry.send(message) {
                        citadel_sdk::logging::error!(target:"citadel","Error sending message to client: {err:?}");
                    }
                }
                None => {
                    citadel_sdk::logging::info!(target:"citadel","Hash map connection not found for TCP uuid: {}", current_tcp_uuid)
                }
            }
        }
    };

    tokio::spawn(connection_read_stream);
}

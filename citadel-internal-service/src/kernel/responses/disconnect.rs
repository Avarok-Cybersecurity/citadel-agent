use crate::kernel::{send_response_to_tcp_client, CitadelWorkspaceService};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{DisconnectNotification, InternalServiceResponse};
use citadel_sdk::prelude::{Disconnect, NetworkError, Ratchet, VirtualTargetType};
use std::sync::atomic::Ordering;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    disconnect: Disconnect,
) -> Result<(), NetworkError> {
    // If disconnect is due to a rejected connection attempt, the existing session should remain valid.
    // These are cases where a duplicate/failed connection attempt was rejected,
    // but the original session is still active and shouldn't be removed.
    let rejected_connection_messages = [
        "Session Already Connected",
        "Preconnect signalled to halt",
    ];

    for reject_msg in &rejected_connection_messages {
        if disconnect.message.contains(reject_msg) {
            citadel_sdk::logging::info!(
                target: "citadel",
                "Disconnect due to '{}' - preserving existing session in server_connection_map",
                reject_msg
            );
            return Ok(());
        }
    }

    if let Some(conn) = disconnect.v_conn_type {
        let (signal, conn_uuid) = match conn {
            VirtualTargetType::LocalGroupServer { session_cid } => {
                let mut server_connection_map = this.server_connection_map.lock().await;
                if let Some(conn) = server_connection_map.remove(&session_cid) {
                    (
                        InternalServiceResponse::DisconnectNotification(DisconnectNotification {
                            cid: session_cid,
                            peer_cid: None,
                            request_id: None,
                        }),
                        conn.associated_tcp_connection.load(Ordering::Relaxed),
                    )
                } else {
                    return Ok(());
                }
            }
            VirtualTargetType::LocalGroupPeer {
                session_cid,
                peer_cid,
            } => {
                if let Some(conn) = this.clear_peer_connection(session_cid, peer_cid).await {
                    (
                        InternalServiceResponse::DisconnectNotification(DisconnectNotification {
                            cid: session_cid,
                            peer_cid: Some(peer_cid),
                            request_id: None,
                        }),
                        conn.associated_tcp_connection.load(Ordering::Relaxed),
                    )
                } else {
                    return Ok(());
                }
            }
            _ => return Ok(()),
        };

        return send_response_to_tcp_client(&this.tcp_connection_map, signal, conn_uuid).await;
    } else {
        citadel_sdk::logging::warn!(target: "citadel", "The disconnect request does not contain a connection type")
    }

    Ok(())
}

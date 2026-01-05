use crate::kernel::{send_response_to_tcp_client, CitadelWorkspaceService};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceResponse, MessageNotification, PeerConnectSuccess,
};
use citadel_sdk::logging::{error, info};
use citadel_sdk::prelude::{NetworkError, PeerChannelCreated, Ratchet};
use futures::StreamExt;
use std::sync::atomic::Ordering;

/// Handle PeerChannelCreated events from the SDK.
///
/// This event is emitted when a P2P channel is successfully established.
/// For INITIATORS: The channel is typically captured by `connect_to_peer_custom` before reaching here.
/// For ACCEPTORS: The channel flows through here and must be stored to enable bidirectional messaging.
///
/// This handler:
/// 1. Extracts session_cid and peer_cid from the channel
/// 2. Adds the peer to the session's peers map (using the send half)
/// 3. Spawns a task to read incoming messages from the receive half
/// 4. Notifies the UI that the P2P connection is established
pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    peer_channel_created: PeerChannelCreated<R>,
) -> Result<(), NetworkError> {
    let channel = *peer_channel_created.channel;
    let session_cid = channel.get_session_cid();
    let peer_cid = channel.get_peer_cid();

    info!(target: "citadel", "[PeerChannelCreated] *** RECEIVED P2P CHANNEL *** session_cid={}, peer_cid={}", session_cid, peer_cid);
    info!(target: "citadel", "[PeerChannelCreated] This is the SDK event indicating successful P2P handshake");

    // Split the channel into send and receive halves
    let (sink, mut stream) = channel.split();

    // Lock the server connection map and add the peer
    let mut server_connection_map = this.server_connection_map.write();

    if let Some(connection) = server_connection_map.get_mut(&session_cid) {
        // Check if peer already exists (initiator may have already added it via connect.rs)
        let peer_existed = connection.peers.contains_key(&peer_cid);

        if peer_existed {
            info!(target: "citadel", "[PeerChannelCreated] Peer {} already in peers map for session {} (initiator path)", peer_cid, session_cid);
            // CRITICAL: Do NOT return early! Even if the peer exists, this channel's receive
            // stream must be spawned. In SIMULTANEOUS_CONNECT, both sides call PeerConnect.
            // The initiator's connect.rs sets up one stream, but this PeerChannelCreated event
            // may carry a DIFFERENT channel from the SDK that also needs its stream consumed.
            //
            // Without spawning this task, the acceptor's incoming messages are lost because
            // the stream is dropped when this function returns.
            info!(target: "citadel", "[PeerChannelCreated] STILL spawning receive stream task to prevent message loss");
        } else {
            // Add the peer connection (acceptor side - no PeerRemote available)
            connection.add_peer_connection_channel_only(peer_cid, sink);
            info!(target: "citadel", "[PeerChannelCreated] Added peer {} to session {} (channel only). Total peers: {}", peer_cid, session_cid, connection.peers.len());
        }

        let associated_tcp_connection = connection
            .associated_localhost_connection
            .load(Ordering::Relaxed);
        let tcp_connection_map = this.tx_to_localhost_clients.clone();
        let server_conn_map = this.server_connection_map.clone();

        drop(server_connection_map);

        // Spawn a task to read incoming messages from the peer
        tokio::spawn(async move {
            info!(target: "citadel", "[P2P-RECV-CHANNEL] *** Starting P2P read stream for LOCAL_CID={} from PEER={} ***", session_cid, peer_cid);
            info!(target: "citadel", "[P2P-RECV-CHANNEL] This stream will receive messages SENT BY peer {}", peer_cid);

            while let Some(message) = stream.next().await {
                info!(target: "citadel", "[PeerChannelCreated] Received P2P message! session={}, peer_cid={}, msg_len={}", session_cid, peer_cid, message.len());

                let notification =
                    InternalServiceResponse::MessageNotification(MessageNotification {
                        message: message.into_buffer(),
                        cid: session_cid,
                        peer_cid,
                        request_id: None,
                    });

                // Get the current associated TCP connection (may have changed via ClaimSession)
                let server_lock = server_conn_map.read();
                let current_tcp_uuid = server_lock
                    .get(&session_cid)
                    .map(|conn| conn.associated_localhost_connection.load(Ordering::Relaxed))
                    .unwrap_or(associated_tcp_connection);
                drop(server_lock);

                // Forward to TCP client
                match tcp_connection_map.read().get(&current_tcp_uuid) {
                    Some(entry) => {
                        if let Err(err) = entry.send(notification) {
                            error!(target: "citadel", "[PeerChannelCreated] Error sending message to client: {err:?}");
                        }
                    }
                    None => {
                        info!(target: "citadel", "[PeerChannelCreated] TCP connection not found for uuid: {}", current_tcp_uuid);
                    }
                }
            }

            info!(target: "citadel", "[PeerChannelCreated] P2P read stream ended for session={} from peer={}", session_cid, peer_cid);
        });

        // Notify the UI that the P2P connection is established
        send_response_to_tcp_client(
            &this.tx_to_localhost_clients,
            InternalServiceResponse::PeerConnectSuccess(PeerConnectSuccess {
                cid: session_cid,
                peer_cid,
                request_id: None,
            }),
            associated_tcp_connection,
        )?;

        Ok(())
    } else {
        error!(target: "citadel", "[PeerChannelCreated] No connection found for session_cid={} in server_connection_map", session_cid);
        Err(NetworkError::Generic(format!(
            "No connection found for session_cid={} in connection map",
            session_cid
        )))
    }
}

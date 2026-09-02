use crate::kernel::session_route::SessionRoute;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceResponse, MessageNotification, PeerConnectSuccess,
};
use citadel_sdk::logging::{error, info, warn};
use citadel_sdk::prelude::{NetworkError, PeerChannelCreated, Ratchet};
use futures::StreamExt;

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
    // Taken before the channel is consumed: this is the acceptor side's only
    // offer of a UDP channel, and a call needs it to avoid running media over the
    // reliable path.
    let udp_rx = peer_channel_created.udp_rx_opt;
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
            info!(target: "citadel", "[PeerChannelCreated] Peer {} already in peers map for session {} - updating with new channel", peer_cid, session_cid);
            // CRITICAL FIX: Always update the sink with the new channel.
            // In reconnection scenarios (e.g., hard disconnect then reconnect), the old sink
            // becomes stale but the peer entry still exists. Without updating, messages
            // sent through the old sink will fail.
            //
            // Also: Do NOT return early! The receive stream must be spawned even if
            // peer exists. In SIMULTANEOUS_CONNECT, both sides call PeerConnect.
            // The initiator's connect.rs sets up one stream, but this PeerChannelCreated event
            // may carry a DIFFERENT channel that also needs its stream consumed.
        }

        // Upserts in place: the sink is refreshed, but a live media session and
        // its UDP transport survive — a blind insert here used to drop the
        // existing entry, whose Drop aborted a mid-call inbound media pump.
        connection.add_peer_connection_channel_only(peer_cid, sink, udp_rx);
        info!(target: "citadel", "[PeerChannelCreated] {} peer {} to session {} (channel only). Total peers: {}",
            if peer_existed { "Updated" } else { "Added" },
            peer_cid, session_cid, connection.peers.len());

        // The third hand-rolled copy of "route to whoever owns this session
        // right now" -- and the only one of the three that was correct. The
        // other two froze the uuid at spawn. One implementation now, so a
        // fourth caller cannot get it wrong: kernel/session_route.rs.
        let route = SessionRoute::new(
            connection.associated_localhost_connection.clone(),
            this.tx_to_localhost_clients.clone(),
        );

        drop(server_connection_map);

        // Spawn a task to read incoming messages from the peer
        let stream_route = route.clone();
        tokio::spawn(async move {
            let route = stream_route;
            info!(target: "citadel", "[P2P-RECV-CHANNEL] *** Starting P2P read stream for LOCAL_CID={} from PEER={} ***", session_cid, peer_cid);
            info!(target: "citadel", "[P2P-RECV-CHANNEL] This stream will receive messages SENT BY peer {}", peer_cid);

            while let Some(message) = stream.next().await {
                info!(target: "citadel", "[PeerChannelCreated] Received P2P message! session={}, peer_cid={}, msg_len={}", session_cid, peer_cid, message.len());

                let notification =
                    InternalServiceResponse::MessageNotification(MessageNotification {
                        message: message.into_buffer().into(),
                        cid: session_cid,
                        peer_cid,
                        request_id: None,
                    });

                // Send only to the one client that owns this session. An
                // earlier version broadcast to every live TCP entry as a
                // workaround for stale-uuid delivery, and that leaked P2P
                // message content to any other session multiplexed through the
                // same internal-service process. If nobody owns it, ILM is the
                // layer that retries.
                if route.send(notification).is_none() {
                    info!(target: "citadel", "[PeerChannelCreated] No localhost connection owns CID {session_cid}; relying on ILM redelivery");
                }
            }

            info!(target: "citadel", "[PeerChannelCreated] P2P read stream ended for session={} from peer={}", session_cid, peer_cid);
        });

        // Notify the UI that the P2P connection is established. Resolved
        // through the same route as the stream above, so a reclaim between the
        // channel being created and this line cannot send the success to the
        // connection that no longer owns the session.
        if route
            .send(InternalServiceResponse::PeerConnectSuccess(
                PeerConnectSuccess {
                    cid: session_cid,
                    peer_cid,
                    request_id: None,
                },
            ))
            .is_none()
        {
            warn!(target: "citadel", "[PeerChannelCreated] No localhost connection owns CID {session_cid} - PeerConnectSuccess dropped");
        }

        Ok(())
    } else {
        error!(target: "citadel", "[PeerChannelCreated] No connection found for session_cid={} in server_connection_map", session_cid);
        Err(NetworkError::generic(format!(
            "No connection found for session_cid={} in connection map",
            session_cid
        )))
    }
}

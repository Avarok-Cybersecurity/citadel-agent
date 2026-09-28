//! The initiator side's P2P read stream, moved verbatim out of `connect.rs`
//! (which had grown past the file limit) with one change: each message is first
//! offered to the session's agent-hosted ILM (kernel/ilm/inbound.rs). A session
//! that has not opted in reaches the raw forward exactly as before.
use crate::kernel::ilm::inbound::dispatch_inbound;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_types::{InternalServiceResponse, MessageNotification};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{PeerChannelRecvHalf, Ratchet};
use futures::StreamExt;
use uuid::Uuid;

pub(crate) fn spawn_read_stream<T, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    peer_cid: u64,
    request_id: Uuid,
    uuid: Uuid,
    mut stream: PeerChannelRecvHalf<R>,
) {
    let hm_for_conn = this.tx_to_localhost_clients.clone();
    let server_conn_map = this.server_connection_map.clone();
    let agent_ilm = this.agent_ilm.clone();

    let connection_read_stream = async move {
        info!(target:"citadel","[P2P-RECV-CONNECT] *** Starting P2P read stream for LOCAL_CID={cid} from PEER={peer_cid} ***");
        info!(target:"citadel","[P2P-RECV-CONNECT] This stream will receive messages SENT BY peer {peer_cid}");
        while let Some(message) = stream.next().await {
            info!(target:"citadel","[P2P-RECV] Received P2P message! cid={cid}, peer_cid={peer_cid}, msg_len={}", message.len());
            let notification = MessageNotification {
                message: message.into_buffer().into(),
                cid,
                peer_cid,
                request_id: Some(request_id),
            };

            // An opted-in session's ILM frames go to the agent's ILM (see
            // kernel/ilm/inbound.rs); everything else takes the raw path below,
            // unchanged.
            dispatch_inbound(&agent_ilm, notification, |notification| {
                let message = InternalServiceResponse::MessageNotification(notification);

                // Get the current associated TCP connection for this session (may have changed via ClaimSession)
                let server_lock = server_conn_map.read();
                let current_tcp_uuid = server_lock
                    .get(&cid)
                    .map(|conn| {
                        conn.associated_localhost_connection
                            .load(std::sync::atomic::Ordering::Relaxed)
                    })
                    .unwrap_or(uuid);
                drop(server_lock);

                info!(target:"citadel","[P2P-RECV] Forwarding to TCP uuid: {current_tcp_uuid}");

                // Send only to that one client. This used to
                // fall back to broadcasting the notification to
                // EVERY live TCP entry when the target uuid was
                // stale — which handed the decrypted body of a
                // P2P message to every other session
                // multiplexed through this agent, including
                // other users' sessions and any other origin
                // holding a socket.
                //
                // The acceptor side (responses/peer_channel_created.rs)
                // already removed exactly this broadcast, for
                // exactly this reason, and the comment there
                // spells it out. The fix was applied to one of
                // the two paths.
                //
                // The stale-uuid case the broadcast was working
                // around is real, and the answer is the one that
                // side settled on: the session's current
                // `associated_localhost_connection` — re-read
                // above, after any ClaimSession — is the sole
                // authoritative destination, and if it is not in
                // the live map then ILM is the layer that
                // retries. Delivering to the wrong client is not
                // a recovery.
                let tcp_map = hm_for_conn.read();

                if let Some(sender) = tcp_map.get(&current_tcp_uuid) {
                    if sender.send(message).is_ok() {
                        info!(target:"citadel","[P2P-RECV] Delivered MessageNotification to {current_tcp_uuid}");
                    } else {
                        warn!(target:"citadel","[P2P-RECV] TCP {current_tcp_uuid} is closed; ILM will retry");
                    }
                } else {
                    warn!(target:"citadel","[P2P-RECV] No live TCP for {current_tcp_uuid}; ILM will retry");
                }

                drop(tcp_map);
            });
        }
        info!(target:"citadel","[P2P-RECV] P2P read stream ended for cid={cid} from peer={peer_cid}");
    };

    tokio::spawn(connection_read_stream);
}

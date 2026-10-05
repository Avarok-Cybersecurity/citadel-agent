//! Reading a peer connection's inbound messages, for a connection this side dialled.

use crate::kernel::ilm::IlmRegistry;
use crate::kernel::session_route::SessionRoute;
use citadel_internal_service_types::{InternalServiceResponse, MessageNotification};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{PeerChannelRecvHalf, Ratchet};
use futures::StreamExt;
use std::sync::Arc;
use uuid::Uuid;

pub(super) fn spawn_read_stream<R: Ratchet>(
    ilm: Arc<IlmRegistry>,
    read_route: Option<SessionRoute>,
    mut stream: PeerChannelRecvHalf<R>,
    cid: u64,
    peer_cid: u64,
    request_id: Option<Uuid>,
) {
    tokio::spawn(async move {
        info!(target:"citadel","[P2P-RECV-CONNECT] *** Starting P2P read stream for LOCAL_CID={cid} from PEER={peer_cid} ***");
        info!(target:"citadel","[P2P-RECV-CONNECT] This stream will receive messages SENT BY peer {peer_cid}");
        while let Some(message) = stream.next().await {
            info!(target:"citadel","[P2P-RECV] Received P2P message! cid={cid}, peer_cid={peer_cid}, msg_len={}", message.len());
            let arrived = MessageNotification {
                message: message.into_buffer().into(),
                cid,
                peer_cid,
                request_id,
            };
            // A hosted account's ILM frames go to its ILM (kernel/ilm).
            let Err(raw) = ilm.feed(arrived) else {
                continue;
            };
            let message = InternalServiceResponse::MessageNotification(raw);

            // To every connection attached to the session, and to
            // nobody else. This once fell back to broadcasting to
            // EVERY live TCP entry when the owner was stale, which
            // handed the decrypted body of a P2P message to every
            // other session multiplexed through this agent. The
            // session's subscriber set, read at send time, is the
            // sole authoritative destination; if nobody is attached,
            // ILM is the layer that retries (kernel/session_route.rs).
            let delivered = read_route
                .as_ref()
                .map(|route| route.send(message))
                .unwrap_or_default();
            if delivered.is_empty() {
                warn!(target:"citadel","[P2P-RECV] No window attached to CID {cid}; ILM will retry");
            } else {
                info!(target:"citadel","[P2P-RECV] Delivered MessageNotification to {delivered:?}");
            }
        }
        info!(target:"citadel","[P2P-RECV] P2P read stream ended for cid={cid} from peer={peer_cid}");
    });
}

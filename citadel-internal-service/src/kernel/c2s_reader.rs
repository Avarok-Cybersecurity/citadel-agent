//! Forwards what the server sends a session to every localhost client attached to it.
//!
//! One reader per C2S channel. Connect starts one; a reconnect starts one for the new
//! channel, since the old one ends with the link it was reading.

use crate::kernel::apps::net::grants::GrantCache;
use crate::kernel::session_route::SessionRoute;
use crate::telemetry::TelemetryUplink;
use citadel_internal_service_types::{InternalServiceResponse, MessageNotification};
use citadel_sdk::prelude::{PeerChannelRecvHalf, Ratchet};
use futures::StreamExt;
use std::sync::Arc;
use uuid::Uuid;

/// `request_id` tags every notification with the Connect that opened the session.
/// `route` resolves the attached connections per message, so windows that attach or
/// drop while the session lives are followed (kernel/session_route.rs). `telemetry` reads
/// each frame first and keeps the answers to the agent's own uploads (telemetry/uplink);
/// `grants` keeps the answers to host egress's questions about installs, and is told the
/// link is up (kernel/apps/net/grants.rs).
pub(crate) fn spawn<R: Ratchet>(
    route: SessionRoute,
    cid: u64,
    mut stream: PeerChannelRecvHalf<R>,
    request_id: Uuid,
    telemetry: Option<Arc<TelemetryUplink>>,
    grants: Option<Arc<GrantCache>>,
) {
    if let Some(grants) = &grants {
        grants.link_up(cid);
    }
    let connection_read_stream = async move {
        while let Some(message) = stream.next().await {
            let message = message.into_buffer();
            if let Some(uplink) = &telemetry {
                if !uplink.on_server_frame(cid, message.as_ref()) {
                    continue;
                }
            }
            if let Some(grants) = &grants {
                if !grants.on_server_frame(cid, message.as_ref()) {
                    continue;
                }
            }
            let message = InternalServiceResponse::MessageNotification(MessageNotification {
                message: message.into(),
                cid,
                peer_cid: 0,
                request_id: Some(request_id),
            });
            if route.send(message).is_empty() {
                citadel_sdk::logging::info!(target:"citadel","No localhost connection is attached to CID {cid}; server message dropped");
            }
        }
    };

    crate::telemetry::span::spawn_in_span(
        crate::telemetry::span::session_span(cid),
        connection_read_stream,
    );
}

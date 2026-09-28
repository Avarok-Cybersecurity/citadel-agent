//! The one decision both P2P read loops make per message (acceptor side in
//! `responses/peer_channel_created.rs`, initiator side in
//! `requests/peer/connect_read_stream.rs`): the session's agent-hosted ILM, or
//! the raw path the loop has always used.
//!
//! The raw path is passed in rather than rebuilt here, so a session that has
//! not opted in -- every session on an agent run without `multi_subscriber` --
//! reaches exactly the code it reached before, with the notification untouched.
use crate::kernel::ilm::kv::LocalDbAccess;
use crate::kernel::ilm::service::{AgentIlmService, Inbound};
use crate::kernel::ilm::transport::IlmPeerLinks;
use citadel_internal_service_connector::messenger::DeliveryTarget;
use citadel_internal_service_types::MessageNotification;
use citadel_sdk::logging::error;

pub(crate) fn dispatch_inbound<D, L, T>(
    agent_ilm: &AgentIlmService<D, L, T>,
    notification: MessageNotification,
    forward_raw: impl FnOnce(MessageNotification),
) where
    D: LocalDbAccess,
    L: IlmPeerLinks,
    T: DeliveryTarget,
{
    let (cid, peer_cid) = (notification.cid, notification.peer_cid);
    match agent_ilm.route_inbound(notification) {
        Inbound::Raw(notification) => forward_raw(notification),
        Inbound::Hosted => {}
        Inbound::Lost => {
            error!(target: "citadel", "[AGENT-ILM] ILM for {cid} has stopped taking input; frame from {peer_cid} dropped unacknowledged, so the peer will resend it");
        }
    }
}

//! The service's side of the apps relay: an arriving internal-HTTP envelope goes to the
//! session's windows that declared `ihttp`, validated, and nowhere else.

use crate::kernel::apps::ihttp::{self, envelope, Count};
use crate::kernel::session_route::deliver;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_types::{
    IhttpFrameNotification, InternalServiceResponse, MessageNotification,
};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

/// What became of an arriving internal-HTTP envelope.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum IhttpDelivered {
    /// Handed to the windows that take it, or dropped and counted.
    Consumed,
    /// No window took a `RequestHead`: this envelope goes back to the caller (`peer_cid`).
    Refused { peer_cid: u64, answer: Vec<u8> },
}

impl<T, R: Ratchet> CitadelWorkspaceService<T, R> {
    /// The session `cid`'s connections, if `connection` is one of them.
    pub(crate) fn attached(&self, cid: u64, connection: Uuid) -> bool {
        self.server_connection_map
            .read()
            .get(&cid)
            .is_some_and(|conn| conn.subscribers.contains(connection))
    }

    /// Handle a delivered message if it is an internal-HTTP envelope, giving it back
    /// untouched if it is not. An envelope is always consumed: a frame is of use now or never,
    /// so one that is malformed, or that no window can take, is dropped and counted.
    pub(crate) fn deliver_ihttp(
        &self,
        notification: MessageNotification,
    ) -> Result<IhttpDelivered, MessageNotification> {
        let counters = &self.apps.ihttp;
        let (cid, peer_cid) = (notification.cid, notification.peer_cid);
        let frame = match envelope::read(&notification.message) {
            envelope::Inbound::Frame(frame) => frame,
            envelope::Inbound::Malformed(why) => {
                counters.add(Count::MalformedIn);
                let dropped = counters.read().malformed_in;
                warn!(target: "citadel", "[IHTTP] {cid}: dropped a malformed frame from {peer_cid} ({dropped} so far): {why}");
                return Ok(IhttpDelivered::Consumed);
            }
            envelope::Inbound::Other => return Err(notification),
        };
        let attached = crate::kernel::membership::subscribers_of(self, cid)
            .map(|subs| subs.members())
            .unwrap_or_default();
        let targets = ihttp::receivers(&attached, &self.client_capabilities.read());
        let response = InternalServiceResponse::IhttpFrameNotification(IhttpFrameNotification {
            cid,
            peer_cid,
            frame: frame.clone(),
            request_id: None,
        });
        if !deliver(&self.tx_to_localhost_clients, &targets, response).is_empty() {
            counters.add(Count::Received);
            return Ok(IhttpDelivered::Consumed);
        }
        counters.add(Count::NoReceiver);
        let dropped = counters.read().no_receiver;
        info!(target: "citadel", "[IHTTP] {cid}: no window takes internal HTTP; frame from {peer_cid} dropped ({dropped} so far)");
        Ok(match ihttp::unclaimed_answer(&frame) {
            Some(answer) => IhttpDelivered::Refused { peer_cid, answer },
            None => IhttpDelivered::Consumed,
        })
    }
}

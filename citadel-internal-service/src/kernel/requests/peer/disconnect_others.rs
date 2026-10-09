//! A disconnect reaches every window attached to the session, not only the one
//! that asked.

use super::disconnect::DisconnectedConnection;
use crate::kernel::session_route::{Clients, SessionRoute};
use crate::kernel::session_subscribers::SessionSubscribers;
use citadel_internal_service_types::{DisconnectNotification, InternalServiceResponse};
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

impl<R: Ratchet> DisconnectedConnection<R> {
    /// The connections attached to the session when this was removed.
    pub fn subscribers(&self) -> &SessionSubscribers {
        match self {
            Self::C2S { subscribers, .. } | Self::P2P { subscribers, .. } => subscribers,
        }
    }

    /// Tell every window but `requester` that the session ended; answers whom.
    /// The requester gets the response instead. The entry is already out of
    /// the map, so the session is gone for all of them whatever the SDK said.
    pub(crate) fn tell_others(
        &self,
        clients: &Clients,
        requester: Uuid,
        cid: u64,
        peer_cid: Option<u64>,
    ) -> Vec<Uuid> {
        let ended = InternalServiceResponse::DisconnectNotification(DisconnectNotification {
            cid,
            peer_cid,
            request_id: None,
            ended_locally: None,
        });
        SessionRoute::new(self.subscribers().clone(), clients.clone())
            .send_to_others(requester, ended)
    }
}

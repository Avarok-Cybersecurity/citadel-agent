//! Tells every window attached to the account.

use crate::kernel::session_route::SessionRoute;
use crate::kernel::supervisor::ports::Reporter;
use crate::kernel::supervisor::types::SupervisorEvent;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{InternalServiceResponse, SupervisorNotification};
use citadel_sdk::prelude::Ratchet;

pub(super) struct WindowReporter<T, R: Ratchet> {
    pub this: CitadelWorkspaceService<T, R>,
    pub cid: u64,
}

impl<T: IOInterface + Sync, R: Ratchet> Reporter for WindowReporter<T, R> {
    fn report(&self, event: SupervisorEvent) {
        let Some(subscribers) = crate::kernel::membership::subscribers_of(&self.this, self.cid)
        else {
            return;
        };
        let (peer_cid, state) = event.wire();
        // Nobody attached is not an error: the next window to attach asks for what it needs.
        SessionRoute::new(subscribers, self.this.tx_to_localhost_clients.clone()).send(
            InternalServiceResponse::SupervisorNotification(SupervisorNotification {
                cid: self.cid,
                peer_cid,
                state,
                request_id: None,
            }),
        );
    }
}

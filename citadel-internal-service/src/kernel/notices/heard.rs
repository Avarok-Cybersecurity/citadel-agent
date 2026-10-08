//! Telling the declared windows whether anything shows the agent's notices.
//!
//! A window that hosts no ILM of its own leaves a hosted message's OS
//! notification to the agent -- but only something attached to the agent shows
//! one: today the menu-bar app, subscribed to the notice plane. Windows and
//! Linux have none. So the windows are told, on declaring
//! (`AgentCapabilities::notices_heard`) and whenever it changes
//! (`NoticesHeardNotification`), and the browser shows what nobody else would.

use crate::kernel::session_route::deliver;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{InternalServiceResponse, NoticesHeardNotification};
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

impl<T: IOInterface + Sync, R: Ratchet> CitadelWorkspaceService<T, R> {
    /// If whether notices are heard changed, tell every window that declared it hosts no ILM.
    pub(crate) fn notices_heard_changed(&self) {
        let Some(heard) = self.notices.heard_changed() else {
            return;
        };
        let windows: Vec<Uuid> = self
            .client_capabilities
            .read()
            .iter()
            .filter(|(_, capabilities)| capabilities.agent_ilm)
            .map(|(connection, _)| *connection)
            .collect();
        let told = InternalServiceResponse::NoticesHeardNotification(NoticesHeardNotification {
            cid: 0,
            heard,
            request_id: None,
        });
        deliver(&self.tx_to_localhost_clients, &windows, told);
    }

    /// `subscriber` joined the notice plane: say so, and say so again when its connection ends.
    ///
    /// The connection's sender closes when the connection handler drops its
    /// receiver, which is after it left the client map -- so by then `is_heard`
    /// already answers without it.
    pub(crate) fn watch_notice_subscriber(&self, subscriber: Uuid) {
        self.notices_heard_changed();
        let Some(sender) = self
            .tx_to_localhost_clients
            .read()
            .get(&subscriber)
            .cloned()
        else {
            return;
        };
        let this = self.clone();
        tokio::task::spawn(async move {
            sender.closed().await;
            this.notices_heard_changed();
        });
    }
}

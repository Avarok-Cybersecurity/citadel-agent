//! Where a session's notifications go **right now**: to every connection
//! attached to it.
//!
//! Two long-lived tasks used to read the owning connection ONCE, at spawn, and
//! close over the `Uuid`: `spawn_tick_updater` (file-transfer progress) and
//! `spawn_group_channel_receiver` (group broadcasts). After a reclaim, every
//! tick and every broadcast went to a connection that no longer existed. This
//! type resolves at send time instead, and since a session can now be attached
//! to several connections (kernel/session_subscribers.rs), it delivers to all
//! of them. A notification is never sent to a connection outside the session:
//! one nobody is listening for is lost, one sent to everybody is a disclosure.

use crate::kernel::session_subscribers::SessionSubscribers;
use citadel_internal_service_types::{
    InternalServiceResponse, SessionRole, SessionRoleNotification,
};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

pub(crate) type Clients = Arc<RwLock<HashMap<Uuid, UnboundedSender<InternalServiceResponse>>>>;

/// A live route to every localhost connection attached to a session.
///
/// Cheap to clone and safe to hold across awaits: it resolves at send time.
#[derive(Clone)]
pub(crate) struct SessionRoute {
    subscribers: SessionSubscribers,
    clients: Clients,
}

impl SessionRoute {
    pub(crate) fn new(subscribers: SessionSubscribers, clients: Clients) -> Self {
        Self {
            subscribers,
            clients,
        }
    }

    /// Deliver to every attached connection; returns the ones it reached.
    ///
    /// Empty means nobody is listening. The members are read, then the client
    /// map, never both locks at once.
    pub(crate) fn send(&self, response: InternalServiceResponse) -> Vec<Uuid> {
        deliver(&self.clients, &self.subscribers.members(), response)
    }

    /// Deliver to every attached connection except `skip` (the one a request's
    /// own response already went to).
    pub(crate) fn send_to_others(
        &self,
        skip: Uuid,
        response: InternalServiceResponse,
    ) -> Vec<Uuid> {
        let others: Vec<Uuid> = self
            .subscribers
            .members()
            .into_iter()
            .filter(|member| *member != skip)
            .collect();
        deliver(&self.clients, &others, response)
    }
}

/// Send `response` to each of `targets` that is still connected.
pub(crate) fn deliver(
    clients: &Clients,
    targets: &[Uuid],
    response: InternalServiceResponse,
) -> Vec<Uuid> {
    // Cloned out of the map before sending, so the lock is not held across sends.
    let senders: Vec<(Uuid, UnboundedSender<InternalServiceResponse>)> = {
        let map = clients.read();
        targets
            .iter()
            .filter_map(|target| map.get(target).map(|tx| (*target, tx.clone())))
            .collect()
    };
    senders
        .into_iter()
        .filter(|(_, tx)| tx.send(response.clone()).is_ok())
        .map(|(target, _)| target)
        .collect()
}

/// Tell each attached connection its role, and each displaced one that it is
/// detached. Called after a membership change that involves more than one
/// connection, so a session nobody else ever joins never sends one.
pub(crate) fn announce_roles(
    clients: &Clients,
    cid: u64,
    subscribers: &SessionSubscribers,
    displaced: &[Uuid],
) {
    let roles = subscribers.roles();
    let attached = u32::try_from(roles.len()).unwrap_or(u32::MAX);
    let notice = |role: SessionRole, attached: u32| {
        InternalServiceResponse::SessionRoleNotification(SessionRoleNotification {
            cid,
            role,
            attached,
            request_id: None,
        })
    };
    for (member, role) in roles {
        deliver(clients, &[member], notice(role, attached));
    }
    for gone in displaced {
        deliver(clients, &[*gone], notice(SessionRole::Detached, 0));
    }
}

#[cfg(test)]
#[path = "session_route_tests.rs"]
mod tests;

//! Where a session's notifications go **right now**.
//!
//! `associated_localhost_connection` is an `Arc<AtomicUuid>` precisely because
//! it is re-pointed while the session lives: a page reload, a tab switch or a
//! `ClaimSession` moves a live session to a different localhost connection
//! without disturbing anything else.
//!
//! Two long-lived tasks read it ONCE, at spawn, and closed over the `Uuid`:
//! `spawn_tick_updater` (file-transfer progress) and
//! `spawn_group_channel_receiver` (group broadcasts). After a reclaim, every
//! tick and every broadcast for that session went to a connection that no
//! longer exists — the map lookup misses, the task logs "Connection not found"
//! and drops it. The file lands on disk and the UI shows a transfer that never
//! finishes; the group stays silent. Nothing errors.
//!
//! The correct pattern already existed one directory over, in
//! `responses/peer_event.rs::send_response_for_session`, which re-resolves the
//! uuid through the CID on every notification. It was never propagated to these
//! two. This is that resolution, extracted so there is one of it.

use citadel_internal_service_types::{AtomicUuid, InternalServiceResponse};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

type Clients = Arc<RwLock<HashMap<Uuid, UnboundedSender<InternalServiceResponse>>>>;

/// A live route to whichever localhost connection currently owns a session.
///
/// Cheap to clone and safe to hold across awaits: it resolves at send time, not
/// at construction time. That is the entire point — a `Uuid` captured here
/// would be the bug this type exists to remove.
#[derive(Clone)]
pub(crate) struct SessionRoute {
    owner: Arc<AtomicUuid>,
    clients: Clients,
}

impl SessionRoute {
    pub(crate) fn new(owner: Arc<AtomicUuid>, clients: Clients) -> Self {
        Self { owner, clients }
    }

    /// The connection that owns this session at this instant.
    pub(crate) fn current_owner(&self) -> Uuid {
        self.owner.load(Ordering::Relaxed)
    }

    /// Deliver, or report that nobody is listening.
    ///
    /// Returns the resolved uuid on success so callers can log WHERE it went;
    /// `None` means the owning connection is gone. Dropping is deliberate: a
    /// notification nobody is listening for is lost, but one sent to everybody
    /// is a disclosure — the same rule `send_response_for_session` follows.
    pub(crate) fn send(&self, response: InternalServiceResponse) -> Option<Uuid> {
        let target = self.current_owner();
        // Cloned out of the map before sending, so the lock is not held across
        // the send.
        let sender = { self.clients.read().get(&target).cloned() };
        sender?.send(response).ok().map(|()| target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use citadel_internal_service_types::MessageSendSuccess;
    use tokio::sync::mpsc::unbounded_channel;

    fn notification() -> InternalServiceResponse {
        InternalServiceResponse::MessageSendSuccess(MessageSendSuccess {
            cid: 1,
            peer_cid: None,
            request_id: None,
        })
    }

    #[test]
    fn a_notification_reaches_the_current_owner() {
        let first = Uuid::from_u128(1);
        let (tx, mut rx) = unbounded_channel();
        let clients: Clients = Arc::new(RwLock::new(HashMap::from([(first, tx)])));
        let route = SessionRoute::new(Arc::new(AtomicUuid::new(first)), clients);

        assert_eq!(route.send(notification()), Some(first));
        assert!(rx.try_recv().is_ok());
    }

    /// The defect. A reclaim re-points the session mid-transfer; every
    /// subsequent tick must follow it.
    #[test]
    fn a_notification_follows_a_reclaim_to_the_new_owner() {
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        let (first_tx, mut first_rx) = unbounded_channel();
        let (second_tx, mut second_rx) = unbounded_channel();
        let clients: Clients = Arc::new(RwLock::new(HashMap::from([
            (first, first_tx),
            (second, second_tx),
        ])));
        let owner = Arc::new(AtomicUuid::new(first));
        let route = SessionRoute::new(owner.clone(), clients);

        assert_eq!(route.send(notification()), Some(first));

        // ClaimSession does exactly this.
        owner.store(second, Ordering::Relaxed);

        assert_eq!(
            route.send(notification()),
            Some(second),
            "the notification did not follow the session to its new owner"
        );
        assert!(
            second_rx.try_recv().is_ok(),
            "the new owner received nothing"
        );
        assert!(
            first_rx.try_recv().is_ok(),
            "the first send should still have landed on the original owner"
        );
        assert!(
            first_rx.try_recv().is_err(),
            "the old owner received a notification sent after the reclaim"
        );
    }

    /// Nobody listening is a drop, not a broadcast and not a panic.
    #[test]
    fn a_missing_owner_drops_rather_than_broadcasting() {
        let present = Uuid::from_u128(1);
        let absent = Uuid::from_u128(9);
        let (tx, mut rx) = unbounded_channel();
        let clients: Clients = Arc::new(RwLock::new(HashMap::from([(present, tx)])));
        let route = SessionRoute::new(Arc::new(AtomicUuid::new(absent)), clients);

        assert_eq!(route.send(notification()), None);
        assert!(
            rx.try_recv().is_err(),
            "a notification for an absent owner was delivered to somebody else"
        );
    }

    /// A closed receiver is the ordinary shape of a dropped tab. It must read
    /// as "nobody listening", not as success.
    #[test]
    fn a_closed_receiver_reports_nobody_listening() {
        let owner_id = Uuid::from_u128(1);
        let (tx, rx) = unbounded_channel::<InternalServiceResponse>();
        drop(rx);
        let clients: Clients = Arc::new(RwLock::new(HashMap::from([(owner_id, tx)])));
        let route = SessionRoute::new(Arc::new(AtomicUuid::new(owner_id)), clients);

        assert_eq!(route.send(notification()), None);
    }
}

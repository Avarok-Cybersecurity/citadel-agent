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

use crate::kernel::session_subscribers::SessionSubscribers;
use citadel_internal_service_types::InternalServiceResponse;
use parking_lot::RwLock;
use std::collections::HashMap;
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
    owner: Arc<SessionSubscribers>,
    clients: Clients,
}

impl SessionRoute {
    pub(crate) fn new(owner: Arc<SessionSubscribers>, clients: Clients) -> Self {
        Self { owner, clients }
    }

    /// Deliver, or report that nobody is listening.
    ///
    /// Returns the resolved uuid on success so callers can log WHERE it went;
    /// `None` means the owning connection is gone. Dropping is deliberate: a
    /// notification nobody is listening for is lost, but one sent to everybody
    /// is a disclosure — the same rule `send_response_for_session` follows.
    pub(crate) fn send(&self, response: InternalServiceResponse) -> Option<Uuid> {
        // Every subscriber of the session: the owner, then any readers (see
        // kernel/session_subscribers.rs). With no readers this is exactly the
        // old single-owner send. Senders are cloned out of the map first, so
        // the lock is not held across the sends.
        let targets: Vec<(Uuid, UnboundedSender<InternalServiceResponse>)> = {
            let clients = self.clients.read();
            self.owner
                .all()
                .into_iter()
                .filter_map(|uuid| clients.get(&uuid).cloned().map(|tx| (uuid, tx)))
                .collect()
        };
        let mut targets = targets.into_iter();
        let (first_uuid, first_tx) = targets.next()?;
        let mut delivered: Option<Uuid> = None;
        // Readers get copies; the owner, first, gets the original.
        for (uuid, tx) in targets {
            // A reader whose connection just closed: ext.rs removes it from the
            // session; nothing else is owed to it.
            if tx.send(response.clone()).is_err() {
                citadel_sdk::logging::debug!(target: "citadel", "reader {uuid} closed before a notification reached it");
            }
        }
        if first_tx.send(response).is_ok() {
            delivered = Some(first_uuid);
        }
        delivered
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use citadel_internal_service_types::MessageSendSuccess;
    use std::sync::atomic::Ordering;
    use tokio::sync::mpsc::unbounded_channel;

    fn notification() -> InternalServiceResponse {
        InternalServiceResponse::MessageSendSuccess(MessageSendSuccess {
            cid: 1,
            peer_cid: None,
            request_id: None,
        })
    }

    #[test]
    fn a_notification_reaches_every_subscriber_of_the_session() {
        // docs/plans/multi-browser-cid.md: the same account open in two browsers
        // must see the same thing, so a notification goes to the owner AND each
        // reader -- and to nobody else.
        let (owner, reader, stranger) =
            (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
        let (owner_tx, mut owner_rx) = unbounded_channel();
        let (reader_tx, mut reader_rx) = unbounded_channel();
        let (stranger_tx, mut stranger_rx) = unbounded_channel();
        let clients: Clients = Arc::new(RwLock::new(HashMap::from([
            (owner, owner_tx),
            (reader, reader_tx),
            (stranger, stranger_tx),
        ])));
        let subscribers = Arc::new(SessionSubscribers::new(owner));
        subscribers.add_reader(reader);
        let route = SessionRoute::new(subscribers, clients);

        assert_eq!(route.send(notification()), Some(owner));
        assert!(owner_rx.try_recv().is_ok(), "the owner received nothing");
        assert!(reader_rx.try_recv().is_ok(), "the reader received nothing");
        assert!(
            stranger_rx.try_recv().is_err(),
            "a connection outside the session received it"
        );
    }

    #[test]
    fn a_notification_reaches_the_current_owner() {
        let first = Uuid::from_u128(1);
        let (tx, mut rx) = unbounded_channel();
        let clients: Clients = Arc::new(RwLock::new(HashMap::from([(first, tx)])));
        let route = SessionRoute::new(Arc::new(SessionSubscribers::new(first)), clients);

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
        let owner = Arc::new(SessionSubscribers::new(first));
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
        let route = SessionRoute::new(Arc::new(SessionSubscribers::new(absent)), clients);

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
        let route = SessionRoute::new(Arc::new(SessionSubscribers::new(owner_id)), clients);

        assert_eq!(route.send(notification()), None);
    }
}

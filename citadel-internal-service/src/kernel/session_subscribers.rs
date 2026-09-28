//! Which localhost connections receive a session's notifications.
//!
//! A session has one **owner**: the connection that drives it, exactly as the
//! `Arc<AtomicUuid>` this replaces. `load` and `store` keep that meaning, so the
//! code that re-points a session on a reclaim or a takeover is unchanged.
//!
//! It may also have **readers**: further connections for the same account that
//! receive everything the owner receives (docs/plans/multi-browser-cid.md). No
//! code adds a reader yet — joining a session is only safe once the agent owns
//! the per-account conversation state — so until then the set is always empty
//! and a session behaves exactly as it did.

use parking_lot::RwLock;
use std::sync::atomic::Ordering;
use uuid::Uuid;

use citadel_internal_service_types::AtomicUuid;

pub(crate) struct SessionSubscribers {
    owner: AtomicUuid,
    /// Never contains the owner; kept in the order they joined, so a promotion
    /// after the owner leaves is deterministic.
    readers: RwLock<Vec<Uuid>>,
}

impl SessionSubscribers {
    pub(crate) fn new(owner: Uuid) -> Self {
        Self {
            owner: AtomicUuid::new(owner),
            readers: RwLock::new(Vec::new()),
        }
    }

    /// The owning connection.
    pub(crate) fn load(&self, ordering: Ordering) -> Uuid {
        self.owner.load(ordering)
    }

    /// Re-point the session to a new owner (reclaim, takeover). A connection
    /// that was a reader and becomes the owner is no longer also a reader.
    pub(crate) fn store(&self, owner: Uuid, ordering: Ordering) {
        self.readers.write().retain(|reader| *reader != owner);
        self.owner.store(owner, ordering);
    }

    /// Add a reader. The owner, or a reader already present, is not added twice.
    ///
    /// No production caller yet: joining is wired in phase 3, once the agent owns
    /// the per-account conversation state (docs/plans/multi-browser-cid.md).
    #[allow(dead_code)]
    pub(crate) fn add_reader(&self, reader: Uuid) {
        if reader == self.load(Ordering::Relaxed) {
            return;
        }
        let mut readers = self.readers.write();
        if !readers.contains(&reader) {
            readers.push(reader);
        }
    }

    /// Everyone who should receive this session's notifications: the owner first.
    pub(crate) fn all(&self) -> Vec<Uuid> {
        let mut all = vec![self.load(Ordering::Relaxed)];
        all.extend(self.readers.read().iter().copied());
        all
    }

    /// The connection the ownership gate should treat as the owner for a
    /// request from `caller`: the caller itself when it subscribes to this
    /// session, the real owner otherwise.
    pub(crate) fn owner_for(&self, caller: Uuid) -> Uuid {
        if self.readers.read().contains(&caller) {
            caller
        } else {
            self.load(Ordering::Relaxed)
        }
    }

    /// A localhost connection closed. A reader leaves the set; an owner with
    /// readers is replaced by the longest-standing reader, so the session stays
    /// reachable. An owner with no readers stays recorded, exactly as before:
    /// the session is kept for a later reclaim.
    pub(crate) fn connection_closed(&self, closed: Uuid) {
        let mut readers = self.readers.write();
        readers.retain(|reader| *reader != closed);
        if self.owner.load(Ordering::Relaxed) == closed && !readers.is_empty() {
            let next = readers.remove(0);
            self.owner.store(next, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn with_no_readers_it_is_the_old_single_owner() {
        let subscribers = SessionSubscribers::new(id(1));
        assert_eq!(subscribers.all(), vec![id(1)]);
        subscribers.store(id(2), Ordering::Relaxed);
        assert_eq!(subscribers.load(Ordering::Relaxed), id(2));
        assert_eq!(subscribers.all(), vec![id(2)]);
        // A caller that is not a subscriber is judged against the real owner.
        assert_eq!(subscribers.owner_for(id(9)), id(2));
        // The owner closing with nobody else leaves it recorded, as before.
        subscribers.connection_closed(id(2));
        assert_eq!(subscribers.load(Ordering::Relaxed), id(2));
    }

    #[test]
    fn readers_receive_everything_and_pass_the_gate() {
        let subscribers = SessionSubscribers::new(id(1));
        subscribers.add_reader(id(2));
        subscribers.add_reader(id(2));
        subscribers.add_reader(id(1));
        assert_eq!(subscribers.all(), vec![id(1), id(2)]);
        assert_eq!(subscribers.owner_for(id(2)), id(2));
        assert_eq!(subscribers.owner_for(id(3)), id(1));
    }

    #[test]
    fn a_closed_reader_leaves_and_a_closed_owner_is_replaced() {
        let subscribers = SessionSubscribers::new(id(1));
        subscribers.add_reader(id(2));
        subscribers.add_reader(id(3));
        subscribers.connection_closed(id(2));
        assert_eq!(subscribers.all(), vec![id(1), id(3)]);
        subscribers.connection_closed(id(1));
        assert_eq!(subscribers.load(Ordering::Relaxed), id(3));
        assert_eq!(subscribers.all(), vec![id(3)]);
    }

    #[test]
    fn a_reader_promoted_to_owner_is_not_also_a_reader() {
        let subscribers = SessionSubscribers::new(id(1));
        subscribers.add_reader(id(2));
        subscribers.store(id(2), Ordering::Relaxed);
        assert_eq!(subscribers.all(), vec![id(2)]);
    }
}

//! The sessions a reconnect gave up on, kept until their accounts sign in again.
//!
//! `ServerReconnectFailed` reaches only the UI attached when the reconnect gives up. With
//! none attached -- a laptop lid closed through a deploy, the tab closed during the outage
//! -- the session simply vanished: the next `GetSessions` listed nothing and said nothing,
//! and the UI could not tell "signed out by the server" from "never signed in". So the
//! give-up is also recorded here, and `GetSessions` reports it (`signed_out`).
//!
//! One entry per CID, so the store is bounded by the accounts this agent has signed in.
//! In memory only, like the sessions themselves.

use citadel_internal_service_types::SignedOutSession;
use parking_lot::RwLock;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct SignedOut(Arc<RwLock<BTreeMap<u64, SignedOutSession>>>);

impl SignedOut {
    /// The reconnect for `cid` gave up with `reason`; a later give-up replaces an earlier.
    pub fn record(&self, cid: u64, username: String, reason: String) {
        let entry = SignedOutSession {
            cid,
            username,
            reason,
        };
        self.0.write().insert(cid, entry);
    }

    /// `cid` has a session again, so it is no longer signed out.
    pub fn clear(&self, cid: u64) {
        self.0.write().remove(&cid);
    }

    /// Every account signed out by its server, in CID order.
    pub fn list(&self) -> Vec<SignedOutSession> {
        self.0.read().values().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::SignedOut;

    #[test]
    fn a_give_up_is_listed_until_the_account_has_a_session_again() {
        let store = SignedOut::default();
        store.record(7, "alice".into(), "no answer".into());
        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            (
                listed[0].cid,
                listed[0].username.as_str(),
                listed[0].reason.as_str()
            ),
            (7, "alice", "no answer")
        );
        store.clear(7);
        assert!(store.list().is_empty());
    }

    #[test]
    fn a_later_give_up_replaces_the_earlier_one() {
        let store = SignedOut::default();
        store.record(7, "alice".into(), "first".into());
        store.record(7, "alice".into(), "second".into());
        let listed = store.list();
        assert_eq!(listed.len(), 1, "one entry per account");
        assert_eq!(listed[0].reason, "second");
    }

    #[test]
    fn clearing_one_account_leaves_the_others() {
        let store = SignedOut::default();
        store.record(7, "alice".into(), "r".into());
        store.record(9, "bob".into(), "r".into());
        store.clear(7);
        let cids: Vec<u64> = store.list().iter().map(|s| s.cid).collect();
        assert_eq!(cids, vec![9]);
    }
}

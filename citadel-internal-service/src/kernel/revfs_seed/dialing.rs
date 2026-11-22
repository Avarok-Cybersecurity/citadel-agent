//! The RE-VFS keys of sessions being dialled, for the node's key source until the session is in
//! the connection map.
//!
//! The server prompts a member to restore each of its groups as soon as the sign-in succeeds,
//! before the agent has recorded the session. A source that read only the map answered those
//! prompts with no key, so a device the account was just recovered on rejoined its groups
//! without the state the server keeps for it, and without their history. A Connect publishes the
//! key here before it dials (`Dialing::publish`); the key is withdrawn when the returned guard is
//! dropped, once the session is recorded or the sign-in has failed.

use citadel_sdk::prelude::RevfsKey;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Default)]
pub(crate) struct Dialing {
    keys: Arc<RwLock<HashMap<u64, RevfsKey>>>,
}

/// Withdraws its key when dropped.
#[must_use = "the key is withdrawn as soon as the guard is dropped"]
pub(crate) struct Published {
    dialing: Dialing,
    cid: u64,
}

impl Dialing {
    /// `key` answers for `cid` until the guard is dropped.
    pub(crate) fn publish(&self, cid: u64, key: RevfsKey) -> Published {
        let _replaced = self.keys.write().insert(cid, key);
        Published {
            dialing: self.clone(),
            cid,
        }
    }

    pub(crate) fn key(&self, cid: u64) -> Option<RevfsKey> {
        self.keys.read().get(&cid).cloned()
    }
}

impl Drop for Published {
    fn drop(&mut self) {
        let _withdrawn = self.dialing.keys.write().remove(&self.cid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> RevfsKey {
        RevfsKey::derive(&[byte; 32], "example.com", 9).expect("a key derives")
    }

    #[test]
    fn a_published_key_answers_until_its_guard_is_dropped() {
        let dialing = Dialing::default();
        assert!(dialing.key(9).is_none());
        let guard = dialing.publish(9, key(1));
        assert_eq!(dialing.key(9).map(|k| k.id()), Some(key(1).id()));
        assert!(dialing.key(10).is_none());
        drop(guard);
        assert!(dialing.key(9).is_none());
    }
}

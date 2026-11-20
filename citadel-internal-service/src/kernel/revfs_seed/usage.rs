//! Who is using an account's RE-VFS key. A re-key (rekey.rs) points the account's key at a
//! retired root while it reads an object, so it runs alone: it starts only when no store, pull
//! or delete of the account is in flight, and while it runs every new one is refused.
//!
//! In memory: a re-key that a restart interrupted resumes from the index, not from here.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// Why a RE-VFS request was refused while the account is being re-keyed.
pub const REKEYING: &str = "The account's files are being sealed again under its new \
     recovery phrase; try again when that has finished";

#[derive(Default)]
struct Busy {
    rekeying: bool,
    in_flight: usize,
}

type Table = Arc<Mutex<HashMap<u64, Busy>>>;

#[derive(Default)]
pub(crate) struct Accounts {
    /// Serialises every read-modify-write of an account's roots and index.
    pub(crate) records: tokio::sync::Mutex<()>,
    busy: Table,
}

/// One store, pull or delete in flight. Clones share it; the last one dropped ends it.
#[derive(Clone)]
pub(crate) struct UseToken(#[allow(dead_code)] Arc<InUse>);

struct InUse {
    busy: Table,
    cid: u64,
}

impl Drop for InUse {
    fn drop(&mut self) {
        let mut busy = self.busy.lock();
        if let Some(state) = busy.get_mut(&self.cid) {
            state.in_flight = state.in_flight.saturating_sub(1);
            if !state.rekeying && state.in_flight == 0 {
                busy.remove(&self.cid);
            }
        }
    }
}

impl std::fmt::Debug for UseToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UseToken")
    }
}

/// A token says nothing about the request it travels with.
impl PartialEq for UseToken {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl Eq for UseToken {}

/// The re-key of one account, under way. Dropping it lets the account's requests in again.
pub(crate) struct Rekeying {
    busy: Table,
    cid: u64,
}

impl Drop for Rekeying {
    fn drop(&mut self) {
        let mut busy = self.busy.lock();
        if let Some(state) = busy.get_mut(&self.cid) {
            state.rekeying = false;
            if state.in_flight == 0 {
                busy.remove(&self.cid);
            }
        }
    }
}

impl Accounts {
    /// A store, pull or delete of `cid` begins; refused while the account is re-keyed.
    pub(crate) fn begin_use(&self, cid: u64) -> Result<UseToken, String> {
        let mut busy = self.busy.lock();
        let state = busy.entry(cid).or_default();
        if state.rekeying {
            return Err(REKEYING.to_string());
        }
        state.in_flight += 1;
        Ok(UseToken(Arc::new(InUse {
            busy: self.busy.clone(),
            cid,
        })))
    }

    /// The re-key of `cid` begins, unless one is under way or a request is still in flight.
    pub(crate) fn begin_rekey(&self, cid: u64) -> Option<Rekeying> {
        let mut busy = self.busy.lock();
        let state = busy.entry(cid).or_default();
        if state.rekeying || state.in_flight > 0 {
            return None;
        }
        state.rekeying = true;
        Some(Rekeying {
            busy: self.busy.clone(),
            cid,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rekey_waits_for_requests_in_flight_and_refuses_new_ones() {
        let accounts = Accounts::default();
        let store = accounts.begin_use(7).unwrap();
        let shared = store.clone();
        assert!(accounts.begin_rekey(7).is_none(), "a store is in flight");
        drop(store);
        assert!(accounts.begin_rekey(7).is_none(), "its clone still is");
        drop(shared);
        let rekey = accounts.begin_rekey(7).expect("nothing in flight");
        assert!(accounts.begin_rekey(7).is_none(), "one re-key at a time");
        assert_eq!(accounts.begin_use(7).unwrap_err(), REKEYING);
        assert!(accounts.begin_use(8).is_ok(), "another account is not held");
        drop(rekey);
        assert!(accounts.begin_use(7).is_ok());
    }
}

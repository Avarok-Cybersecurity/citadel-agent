//! Each live peer connection's path cell, for decisions that depend on the path it is on now:
//! whether a call may start (requests/media/path_gate.rs) and how many bytes rode TURN
//! (telemetry/turn_ledger). Registered once per connection, where it comes up, on both the
//! dialling and the accepting side; a session's go when it ends.

use super::CitadelWorkspaceService;
use citadel_sdk::prelude::{P2pPathCell, PathClass, Ratchet};
use parking_lot::RwLock;
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct PeerPaths(RwLock<HashMap<(u64, u64), P2pPathCell>>);

impl PeerPaths {
    /// Whether `cid`'s connection to `peer_cid` may carry `class` on its current path; `None`
    /// without such a connection.
    pub(crate) fn permits(&self, cid: u64, peer_cid: u64, class: PathClass) -> Option<bool> {
        self.0
            .read()
            .get(&(cid, peer_cid))
            .map(|cell| cell.permits(class))
    }

    /// Every connection of `cid`, or only the one to `peer_cid`.
    fn forget(&self, cid: u64, peer_cid: Option<u64>) {
        forget(&mut self.0.write(), cid, peer_cid);
    }
}

/// Drops `cid`'s entries, or only its one for `peer_cid`.
fn forget<V>(map: &mut HashMap<(u64, u64), V>, cid: u64, peer_cid: Option<u64>) {
    map.retain(|(session, peer), _| *session != cid || peer_cid.is_some_and(|p| p != *peer));
}

impl<T, R: Ratchet> CitadelWorkspaceService<T, R> {
    /// `cid`'s connection to `peer_cid` is up on `cell`; it replaces any earlier one.
    pub(crate) fn peer_path_up(&self, cid: u64, peer_cid: u64, cell: P2pPathCell) {
        self.peer_paths
            .0
            .write()
            .insert((cid, peer_cid), cell.clone());
        self.telemetry_track_turn(cid, peer_cid, cell);
    }

    /// The session ended, or only its connection to `peer_cid`: their cells go with them.
    pub(crate) fn peer_paths_forget(&self, cid: u64, peer_cid: Option<u64>) {
        self.peer_paths.forget(cid, peer_cid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forgetting_one_peer_keeps_the_sessions_others() {
        let mut map: HashMap<(u64, u64), ()> = [((1, 7), ()), ((1, 8), ()), ((2, 7), ())].into();
        forget(&mut map, 1, Some(7));
        assert!(!map.contains_key(&(1, 7)) && map.contains_key(&(1, 8)));
        forget(&mut map, 1, None);
        assert_eq!(map.keys().collect::<Vec<_>>(), vec![&(2, 7)]);
    }
}

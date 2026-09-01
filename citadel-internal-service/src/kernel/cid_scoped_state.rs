//! Pruning of kernel-lifetime state that is keyed by CID.
//!
//! Three maps on the kernel are keyed by a CID pair and outlive the session
//! they describe: the two pending peer-signal maps and the username cache.
//! Nothing removed entries from them. They are inserted when a peer request
//! arrives and removed only when the local user explicitly answers it, so an
//! ignored request — the common case — stayed for the life of the process.
//!
//! That is a slow leak, but the sharper problem is staleness. A CID is
//! permanent per account, so an entry survives logout and reconnection and can
//! later be matched against a request the sender long since abandoned. It even
//! survives deregistration: the account is deleted and its pending signals are
//! still in memory.
//!
//! A CID can appear on either side of the key — as the local session for
//! requests addressed to it, and as the peer for requests it sent to others —
//! and both are dead once the session is gone, so both are pruned.

use crate::kernel::CitadelWorkspaceService;
use citadel_sdk::prelude::Ratchet;

/// What a single prune removed, so callers can log it and tests can assert it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PrunedCidState {
    pub pending_connects: usize,
    pub pending_registrations: usize,
    pub cached_usernames: usize,
}

impl PrunedCidState {
    pub fn total(&self) -> usize {
        self.pending_connects + self.pending_registrations + self.cached_usernames
    }
}

impl<T, R: Ratchet> CitadelWorkspaceService<T, R> {
    /// Drop CID-keyed kernel entries that a teardown has just invalidated.
    ///
    /// `peer_cid == None` is a session (C2S) teardown: everything mentioning
    /// `cid` on either side of the key is dead. `Some(peer)` is a P2P-only
    /// teardown, where the session survives — so only that pair goes, in both
    /// orderings. Pruning by `cid` there would discard pending requests from
    /// unrelated peers that are still perfectly live.
    pub fn prune_cid_scoped_state(&self, cid: u64, peer_cid: Option<u64>) -> PrunedCidState {
        let touches = |key: &(u64, u64)| match peer_cid {
            None => key.0 == cid || key.1 == cid,
            Some(peer) => *key == (cid, peer) || *key == (peer, cid),
        };

        let mut pruned = PrunedCidState::default();
        {
            let mut m = self.pending_peer_connect_signals.write();
            let before = m.len();
            m.retain(|k, _| !touches(k));
            pruned.pending_connects = before - m.len();
        }
        {
            let mut m = self.pending_peer_registrations.write();
            let before = m.len();
            m.retain(|k, _| !touches(k));
            pruned.pending_registrations = before - m.len();
        }
        {
            let mut m = self.peer_username_cache.write();
            let before = m.len();
            m.retain(|k, _| !touches(k));
            pruned.cached_usernames = before - m.len();
        }
        pruned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use citadel_sdk::prelude::{PeerConnectionType, PeerSignal, StackedRatchet};

    type Svc = CitadelWorkspaceService<
        citadel_internal_service_connector::io_interface::in_memory::InMemoryInterface,
        StackedRatchet,
    >;

    fn signal(a: u64, b: u64) -> PeerSignal {
        PeerSignal::PostRegister {
            peer_conn_type: PeerConnectionType::LocalGroupPeer {
                session_cid: a,
                peer_cid: b,
            },
            inviter_username: "inviter".to_string(),
            invitee_username: None,
            ticket_opt: None,
            invitee_response: None,
        }
    }

    fn seeded() -> Svc {
        let (_c, svc): (_, Svc) = CitadelWorkspaceService::new_in_memory();
        {
            let mut m = svc.pending_peer_registrations.write();
            m.insert((1, 2), signal(1, 2)); // 1 is the local session
            m.insert((3, 1), signal(3, 1)); // 1 appears as the peer
            m.insert((3, 4), signal(3, 4)); // unrelated
        }
        {
            let mut m = svc.pending_peer_connect_signals.write();
            m.insert((1, 2), signal(1, 2));
            m.insert((3, 4), signal(3, 4));
        }
        {
            let mut m = svc.peer_username_cache.write();
            m.insert((1, 2), "bob".into());
            m.insert((3, 4), "carol".into());
        }
        svc
    }

    /// A session teardown must clear the CID from BOTH sides of the key: entries
    /// where it is the local session, and entries where another session is
    /// holding a request from it.
    #[test]
    fn session_teardown_prunes_both_sides_and_spares_others() {
        let svc = seeded();
        let pruned = svc.prune_cid_scoped_state(1, None);
        assert_eq!(pruned.pending_registrations, 2, "both (1,2) and (3,1)");
        assert_eq!(pruned.pending_connects, 1);
        assert_eq!(pruned.cached_usernames, 1);

        assert_eq!(svc.pending_peer_registrations.read().len(), 1);
        assert!(svc.pending_peer_registrations.read().contains_key(&(3, 4)));
        assert!(svc.peer_username_cache.read().contains_key(&(3, 4)));
    }

    /// A P2P-only disconnect leaves the session alive, so pruning by CID would
    /// discard live requests from unrelated peers. Only the pair goes.
    #[test]
    fn p2p_teardown_prunes_only_that_pair() {
        let svc = seeded();
        let pruned = svc.prune_cid_scoped_state(1, Some(2));
        assert_eq!(pruned.pending_registrations, 1);
        assert_eq!(pruned.total(), 3, "one entry per map for the (1,2) pair");

        let regs = svc.pending_peer_registrations.read();
        assert!(
            regs.contains_key(&(3, 1)),
            "1's request from 3 is untouched"
        );
        assert!(regs.contains_key(&(3, 4)), "unrelated pair untouched");
    }

    /// The leak this exists to close: an ignored peer request outlives the
    /// session, and outlived even deregistration.
    #[test]
    fn nothing_survives_a_deregistration() {
        let svc = seeded();
        svc.prune_cid_scoped_state(1, None);
        svc.prune_cid_scoped_state(3, None);
        svc.prune_cid_scoped_state(4, None);
        assert_eq!(svc.pending_peer_registrations.read().len(), 0);
        assert_eq!(svc.pending_peer_connect_signals.read().len(), 0);
        assert_eq!(svc.peer_username_cache.read().len(), 0);
    }
}

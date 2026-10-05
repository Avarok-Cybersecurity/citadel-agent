//! What each peer input changes, and the reports and commands it provokes at once. What is
//! merely *due* (probes, dials, upgrades) is `drive`'s; the server link is `link`'s.

use super::super::types::{Cid, Command, DialOutcome, Input, Millis, SupervisorEvent};
use super::Core;
use citadel_internal_service_types::P2pPathReport;

impl Core {
    pub(super) fn handle(&mut self, input: Input, now: Millis) -> Vec<Command> {
        match input {
            Input::Tick { .. } => self.settle(now),
            Input::NetworkChanged { .. } => return self.network_changed(),
            Input::ServerProbe { outcome, .. } => return self.probed(outcome),
            Input::ServerLink { state, .. } => return self.link_changed(state, now),
            Input::ForceFailed { .. } => return self.force_failed(now),
            Input::PeerLost { peer, .. } => self.lost(peer, now),
            Input::PeerReleased { peer, .. } => self.released(peer, now),
            Input::PeerUp { peer, path, .. } => return self.up(peer, path, now),
            Input::PeerPath { peer, path, .. } => self.path_changed(peer, path, now),
            Input::DialResult { peer, outcome, .. } => return self.dialed(peer, outcome, now),
            Input::Backlog { peer, pending } => self.peer(peer).backlog = pending,
            Input::Interest { peer, until } => {
                let entry = self.peer(peer);
                entry.interest_until =
                    Some(entry.interest_until.map_or(until, |had| had.max(until)));
            }
        }
        Vec::new()
    }

    /// A path that has held for `stable_after` forgets its upgrade backoff (the dial backoff
    /// resets in `lost`, when a stable connection is next lost).
    fn settle(&mut self, now: Millis) {
        let stable = self.policy.stable_after;
        for peer in self.peers.values_mut() {
            let held = peer.connected && peer.since.is_some_and(|since| now.since(since) >= stable);
            if held && peer.path != Some(P2pPathReport::ServerRelay) {
                peer.upgrade.attempts = 0;
            }
        }
    }

    pub(super) fn lost(&mut self, peer: Cid, now: Millis) {
        let (policy, mut rng) = (self.policy, self.rng);
        let entry = self.peer(peer);
        if entry.connected {
            // Lost soon after it came up means flapping: wait the backoff before redialling.
            let stable = entry
                .since
                .is_some_and(|since| now.since(since) >= policy.stable_after);
            if stable {
                entry.dial.attempts = 0;
                entry.dial.next_at = now;
            } else {
                entry.dial.next_at = now.after(rng.delay(
                    policy.dial_backoff,
                    entry.dial.attempts,
                    policy.jitter_permille,
                ));
                entry.dial.attempts = entry.dial.attempts.saturating_add(1);
            }
            entry.lost_at = Some(now);
        }
        entry.connected = false;
        entry.path = None;
        entry.since = None;
        entry.dial.in_flight = false;
        self.rng = rng;
    }

    fn released(&mut self, peer: Cid, now: Millis) {
        self.lost(peer, now);
        let entry = self.peer(peer);
        entry.lost_at = None;
        entry.interest_until = None;
    }

    fn up(&mut self, peer: Cid, path: P2pPathReport, now: Millis) -> Vec<Command> {
        let entry = self.peer(peer);
        entry.connected = true;
        entry.since = Some(now);
        entry.dial.in_flight = false;
        entry.dialing_reported = false;
        entry.degraded_reported = false;
        let restored = entry.lost_at.take().is_some();
        self.path_changed(peer, path, now);
        if restored {
            vec![Command::Report(SupervisorEvent::PeerRestored {
                peer_cid: peer,
                path,
            })]
        } else {
            Vec::new()
        }
    }

    fn path_changed(&mut self, peer: Cid, path: P2pPathReport, now: Millis) {
        let (policy, mut rng) = (self.policy, self.rng);
        let entry = self.peer(peer);
        let was_relay = entry.on_relay();
        entry.path = Some(path);
        if path == P2pPathReport::ServerRelay && !was_relay {
            // Newly on the relay: the SDK's own campaign gets the first chance.
            entry.upgrade.next_at = now.after(rng.delay(
                policy.upgrade_backoff,
                entry.upgrade.attempts,
                policy.jitter_permille,
            ));
        }
        self.rng = rng;
    }

    fn dialed(&mut self, peer: Cid, outcome: DialOutcome, now: Millis) -> Vec<Command> {
        let (policy, mut rng) = (self.policy, self.rng);
        let entry = self.peer(peer);
        entry.dial.in_flight = false;
        if outcome == DialOutcome::Connected {
            // The path report that refines this may arrive after the dial's own answer; until
            // it does, a connection the SDK delivers is on the server relay.
            if entry.connected {
                return Vec::new();
            }
            return self.up(peer, P2pPathReport::ServerRelay, now);
        }
        entry.dial.next_at = now.after(rng.delay(
            policy.dial_backoff,
            entry.dial.attempts,
            policy.jitter_permille,
        ));
        entry.dial.attempts = entry.dial.attempts.saturating_add(1);
        let first = !std::mem::replace(&mut entry.degraded_reported, true);
        self.rng = rng;
        if first {
            vec![Command::Report(SupervisorEvent::Degraded {
                peer_cid: peer,
            })]
        } else {
            Vec::new()
        }
    }
}

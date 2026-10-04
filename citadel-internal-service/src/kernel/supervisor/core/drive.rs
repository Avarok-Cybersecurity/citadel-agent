//! What is due now: the next probe, the dials, the path upgrades.

use super::super::types::{Command, LinkStatus, Millis, SupervisorEvent};
use super::Core;

impl Core {
    pub(super) fn drive(&mut self, now: Millis) -> Vec<Command> {
        let mut commands = Vec::new();
        if self.link != LinkStatus::Up || self.forcing {
            return commands;
        }
        let armed = *self
            .probe
            .next_at
            .get_or_insert(now.after(self.policy.probe_interval));
        if !self.probe.in_flight && armed <= now {
            self.probe.in_flight = true;
            commands.push(Command::ProbeServer);
        }
        let policy = self.policy;
        let mut rng = self.rng;
        for (&cid, peer) in self.peers.iter_mut() {
            if policy.dial_peers
                && !peer.connected
                && !peer.dial.in_flight
                && peer.wanted(now, policy.redial_window)
                && peer.dial.next_at <= now
            {
                peer.dial.in_flight = true;
                if !std::mem::replace(&mut peer.dialing_reported, true) {
                    commands.push(Command::Report(SupervisorEvent::PeerDialing {
                        peer_cid: cid,
                    }));
                }
                commands.push(Command::DialPeer { peer: cid });
            }
            if peer.on_relay() && peer.in_use(now) && peer.upgrade.next_at <= now {
                peer.upgrade.attempts = peer.upgrade.attempts.saturating_add(1);
                peer.upgrade.next_at = now.after(rng.delay(
                    policy.upgrade_backoff,
                    peer.upgrade.attempts,
                    policy.jitter_permille,
                ));
                commands.push(Command::UpgradePath {
                    peer: cid,
                    restore_udp: true,
                });
            }
        }
        self.rng = rng;
        commands
    }
}

pub(super) fn next_wake(core: &Core) -> Option<Millis> {
    if core.link != LinkStatus::Up || core.forcing {
        return None;
    }
    let probe = (!core.probe.in_flight)
        .then_some(core.probe.next_at)
        .flatten();
    let peers = core.peers.values().flat_map(|peer| {
        let dial = (core.policy.dial_peers
            && !peer.connected
            && !peer.dial.in_flight
            && peer.wanted(core.now, core.policy.redial_window))
        .then_some(peer.dial.next_at);
        let upgrade = (peer.on_relay() && peer.in_use(core.now)).then_some(peer.upgrade.next_at);
        [dial, upgrade]
    });
    probe.into_iter().chain(peers.flatten()).min()
}

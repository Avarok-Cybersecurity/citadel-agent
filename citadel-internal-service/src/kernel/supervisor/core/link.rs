//! The server link: probing it, the network changing under it, and it dropping or ending.

use super::super::types::{
    Cid, Command, HealCause, LinkStatus, Millis, ProbeOutcome, SupervisorEvent,
};
use super::Core;

impl Core {
    pub(super) fn network_changed(&mut self) -> Vec<Command> {
        self.probe.missed = 0;
        self.after_change = true;
        let mut commands = vec![Command::RebindTransports];
        if self.link == LinkStatus::Up && !self.forcing {
            if self.probe.in_flight {
                self.probe.recheck = true;
            } else {
                self.probe.in_flight = true;
                commands.push(Command::ProbeServer);
            }
        }
        commands
    }

    pub(super) fn probed(&mut self, outcome: ProbeOutcome) -> Vec<Command> {
        self.probe.in_flight = false;
        if self.link != LinkStatus::Up {
            return Vec::new();
        }
        if std::mem::take(&mut self.probe.recheck) {
            self.probe.in_flight = true;
            return vec![Command::ProbeServer];
        }
        match outcome {
            ProbeOutcome::Ok { .. } => {
                self.probe.missed = 0;
                self.probe.next_at = Some(self.now.after(self.policy.probe_interval));
                let mut commands = self.rearm_relayed();
                if std::mem::take(&mut self.healing) {
                    commands.push(Command::Report(SupervisorEvent::Healed));
                }
                commands
            }
            ProbeOutcome::Error => {
                self.probe.next_at = Some(self.now.after(self.policy.probe_interval));
                Vec::new()
            }
            ProbeOutcome::Timeout => {
                self.probe.missed += 1;
                if self.probe.missed < self.policy.missed_probes {
                    self.probe.in_flight = true;
                    return vec![Command::ProbeServer];
                }
                self.probe.missed = 0;
                self.after_change = false;
                self.forcing = true;
                let mut commands = vec![Command::ForceReconnect];
                if !std::mem::replace(&mut self.healing, true) {
                    commands.push(Command::Report(SupervisorEvent::Healing {
                        cause: HealCause::ProbesMissed,
                    }));
                }
                commands
            }
        }
    }

    /// The server answered after a network change: its link survived, so the paths that fell
    /// back to the relay may be able to climb off it from the new address.
    fn rearm_relayed(&mut self) -> Vec<Command> {
        if !std::mem::take(&mut self.after_change) {
            return Vec::new();
        }
        let (now, policy) = (self.now, self.policy);
        let mut commands = Vec::new();
        for (cid, peer) in self.peers.iter_mut().filter(|(_, p)| p.on_relay()) {
            peer.upgrade.attempts = 0;
            peer.upgrade.next_at = now.after(self.rng.delay(
                policy.upgrade_backoff,
                0,
                policy.jitter_permille,
            ));
            commands.push(Command::UpgradePath {
                peer: *cid,
                restore_udp: true,
            });
        }
        commands
    }

    pub(super) fn link_changed(&mut self, state: LinkStatus, now: Millis) -> Vec<Command> {
        match state {
            LinkStatus::Up => {
                self.link = LinkStatus::Up;
                self.forcing = false;
                self.probe = Default::default();
                self.probe.next_at = Some(now.after(self.policy.probe_interval));
                std::mem::take(&mut self.healing)
                    .then_some(Command::Report(SupervisorEvent::Healed))
                    .into_iter()
                    .collect()
            }
            LinkStatus::Reconnecting => self.lose_link(HealCause::LinkLost),
            LinkStatus::Ended => {
                self.link = LinkStatus::Ended;
                self.peers.clear();
                Vec::new()
            }
        }
    }

    /// The link could not be ended: it is as it was, and still unproven. Probing resumes,
    /// and `Healed` waits for a probe the server answers.
    pub(super) fn force_failed(&mut self, now: Millis) -> Vec<Command> {
        self.forcing = false;
        self.probe = Default::default();
        self.probe.next_at = Some(now.after(self.policy.probe_interval));
        Vec::new()
    }

    /// The link is gone: every connected peer went with it.
    fn lose_link(&mut self, cause: HealCause) -> Vec<Command> {
        self.link = LinkStatus::Reconnecting;
        self.forcing = false;
        self.probe = Default::default();
        let now = self.now;
        let connected: Vec<Cid> = self
            .peers
            .iter()
            .filter(|(_, p)| p.connected)
            .map(|(cid, _)| *cid)
            .collect();
        for peer in connected {
            self.lost(peer, now);
        }
        let mut commands = Vec::new();
        if !std::mem::replace(&mut self.healing, true) {
            commands.push(Command::Report(SupervisorEvent::Healing { cause }));
        }
        commands
    }
}

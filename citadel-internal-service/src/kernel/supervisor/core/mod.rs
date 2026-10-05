//! The supervisor's decisions, as a deterministic state machine. No I/O, no clock, no
//! tasks: `step` takes an input stamped with the time it happened and returns commands.
//! One per hosted account. Everything that touches the world is in `adapters` and `shell`.

mod drive;
mod handlers;
mod link;
mod peer;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_link;
#[cfg(test)]
mod tests_paths;
#[cfg(test)]
mod tests_peer_redial;
#[cfg(test)]
mod tests_peers;
#[cfg(test)]
mod tests_stale_drop;

use super::policy::SupervisorPolicy;
use super::types::{Cid, Command, Input, LinkStatus, Millis};
use peer::{Peer, Rng};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default)]
struct Probe {
    /// `None` until the first input says what time it is.
    next_at: Option<Millis>,
    in_flight: bool,
    missed: u32,
    /// The network changed while a probe was out: its answer predates the change.
    recheck: bool,
}

pub struct Core {
    policy: SupervisorPolicy,
    link: LinkStatus,
    /// The latest time any input carried.
    now: Millis,
    probe: Probe,
    peers: BTreeMap<Cid, Peer>,
    rng: Rng,
    /// A `Healing` report is outstanding; a link that comes back, or a probe that is
    /// answered, closes it with `Healed`.
    healing: bool,
    /// `ForceReconnect` is out and not yet answered by the link dropping or failing.
    forcing: bool,
    /// The network changed and no probe has since shown the link alive.
    after_change: bool,
}

impl Core {
    /// `cid` only varies the jitter between accounts.
    pub fn new(policy: SupervisorPolicy, cid: Cid) -> Self {
        Self {
            policy,
            link: LinkStatus::Up,
            now: Millis(0),
            probe: Probe::default(),
            peers: BTreeMap::new(),
            rng: Rng::seeded(policy.jitter_seed, cid),
            healing: false,
            forcing: false,
            after_change: false,
        }
    }

    pub fn step(&mut self, input: Input) -> Vec<Command> {
        if self.link == LinkStatus::Ended {
            return Vec::new();
        }
        let now = match input {
            Input::Tick { now }
            | Input::NetworkChanged { now }
            | Input::ServerProbe { now, .. }
            | Input::ServerLink { now, .. }
            | Input::ForceFailed { now }
            | Input::PeerLost { now, .. }
            | Input::PeerReleased { now, .. }
            | Input::PeerUp { now, .. }
            | Input::PeerPath { now, .. }
            | Input::DialResult { now, .. } => now,
            // These carry no time of their own: they are acted on at the last time seen.
            Input::Backlog { .. } | Input::Interest { .. } => self.now,
        };
        self.now = now;
        let mut commands = self.handle(input, now);
        commands.extend(self.drive(now));
        commands
    }

    /// The earliest time something becomes due without any input, for the shell to sleep to.
    pub fn next_wake(&self) -> Option<Millis> {
        drive::next_wake(self)
    }

    fn peer(&mut self, peer: Cid) -> &mut Peer {
        let now = self.now;
        self.peers.entry(peer).or_insert_with(|| Peer::new(now))
    }
}

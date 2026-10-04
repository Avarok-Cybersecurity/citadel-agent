//! The loop that joins the core to its ports. It holds no decisions: it turns what happens
//! into `Input`s, calls `Core::step`, and does what the commands say (`shell_effects`).

use super::core::Core;
use super::policy::SupervisorPolicy;
use super::ports::{Backlog, Clock, NetworkWatch, Paths, PeerDialer, Reporter, ServerLink};
use super::types::{Cid, DialOutcome, Input, LinkStatus, Millis, ProbeOutcome};
use citadel_internal_service_types::P2pPathReport;
use citadel_sdk::logging::warn;
use futures::FutureExt;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task::{AbortHandle, JoinSet};

pub(super) const LOG: &str = "citadel::supervisor";

/// What the agent's own code tells the supervisor. The time is stamped on arrival.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Signal {
    Link(LinkStatus),
    PeerUp(Cid, P2pPathReport),
    PeerPath(Cid, P2pPathReport),
    PeerLost(Cid),
    /// The user ended it on purpose; it is not redialled.
    PeerReleased(Cid),
    /// A window has `peer` open for `ttl`.
    Interest {
        peer: Cid,
        ttl: Duration,
    },
}

pub(crate) struct Ports {
    pub clock: Arc<dyn Clock>,
    pub link: Arc<dyn ServerLink>,
    pub dialer: Arc<dyn PeerDialer>,
    pub paths: Arc<dyn Paths>,
    pub backlog: Arc<dyn Backlog>,
    pub reporter: Arc<dyn Reporter>,
}

/// Answers from effects that were started earlier.
///
/// Each carries the time it came back, read when the effect finished rather than when the
/// loop got round to it, so a busy loop cannot make a probe look slower or a dial later.
pub(super) enum Done {
    Probe(Millis, ProbeOutcome),
    Dial(Millis, Cid, DialOutcome),
    ForceFailed(Millis),
    Backlog(HashMap<Cid, u32>),
}

pub(super) struct Shell {
    pub(super) core: Core,
    pub(super) policy: SupervisorPolicy,
    pub(super) ports: Ports,
    pub(super) effects: JoinSet<()>,
    pub(super) done: UnboundedSender<Done>,
    pub(super) probe: Option<AbortHandle>,
    /// Peers the last backlog read reported non-empty, so one that emptied is told so.
    pub(super) queued: Vec<Cid>,
    /// A refused path call is said once, not every time it is asked.
    pub(super) said_refused: Vec<&'static str>,
}

/// When a window's interest ends: as it asked, but never later than the ceiling allows, so a
/// window that has gone cannot pin a peer connected.
pub(super) fn interest_until(now: Millis, ttl: Duration, ceiling: Duration) -> Millis {
    now.after(ttl.min(ceiling))
}

/// Runs until `signals` closes or the session ends.
pub(crate) async fn run(
    core: Core,
    policy: SupervisorPolicy,
    mut network: Box<dyn NetworkWatch>,
    ports: Ports,
    mut signals: UnboundedReceiver<Signal>,
) {
    let (done_tx, mut done_rx) = unbounded_channel();
    let mut next_backlog = ports.clock.now();
    let mut network_open = true;
    let mut shell = Shell::new(core, policy, ports, done_tx);
    loop {
        let clock = shell.ports.clock.clone();
        let tick = match shell.core.next_wake() {
            Some(at) => clock.sleep_until(at).boxed(),
            None => futures::future::pending().boxed(),
        };
        let backlog_due = clock.sleep_until(next_backlog);
        let inputs = tokio::select! {
            signal = signals.recv() => match signal {
                Some(signal) => shell.signal(signal),
                None => return,
            },
            Some(done) = done_rx.recv() => shell.finished(done),
            change = network.next_change(), if network_open => match change {
                Some(()) => vec![Input::NetworkChanged { now: clock.now() }],
                None => {
                    warn!(target: LOG, "the network watch ended; address changes will not be noticed");
                    network_open = false;
                    Vec::new()
                }
            },
            () = tick => vec![Input::Tick { now: clock.now() }],
            () = backlog_due => {
                next_backlog = clock.now().after(policy.backlog_poll);
                shell.read_backlog();
                Vec::new()
            }
        };
        for input in inputs {
            let ended = matches!(
                input,
                Input::ServerLink {
                    state: LinkStatus::Ended,
                    ..
                }
            );
            shell.apply(input);
            if ended {
                return;
            }
        }
    }
}

impl Shell {
    fn new(
        core: Core,
        policy: SupervisorPolicy,
        ports: Ports,
        done: UnboundedSender<Done>,
    ) -> Self {
        Self {
            core,
            policy,
            ports,
            effects: JoinSet::new(),
            done,
            probe: None,
            queued: Vec::new(),
            said_refused: Vec::new(),
        }
    }

    fn now(&self) -> Millis {
        self.ports.clock.now()
    }

    fn signal(&mut self, signal: Signal) -> Vec<Input> {
        let now = self.now();
        let input = match signal {
            Signal::Link(state) => {
                // A probe out for the old link says nothing about the new one.
                if let Some(probe) = self.probe.take() {
                    probe.abort();
                }
                Input::ServerLink { now, state }
            }
            Signal::PeerUp(peer, path) => Input::PeerUp { now, peer, path },
            Signal::PeerPath(peer, path) => Input::PeerPath { now, peer, path },
            Signal::PeerLost(peer) => Input::PeerLost { now, peer },
            Signal::PeerReleased(peer) => Input::PeerReleased { now, peer },
            Signal::Interest { peer, ttl } => Input::Interest {
                peer,
                until: interest_until(now, ttl, self.policy.interest_ceiling),
            },
        };
        vec![input]
    }

    fn finished(&mut self, done: Done) -> Vec<Input> {
        match done {
            Done::Probe(now, outcome) => vec![Input::ServerProbe { now, outcome }],
            Done::Dial(now, peer, outcome) => vec![Input::DialResult { now, peer, outcome }],
            Done::ForceFailed(now) => vec![Input::ForceFailed { now }],
            Done::Backlog(pending) => {
                let mut inputs: Vec<Input> = pending
                    .iter()
                    .map(|(&peer, &pending)| Input::Backlog { peer, pending })
                    .collect();
                inputs.extend(
                    self.queued
                        .iter()
                        .filter(|peer| !pending.contains_key(peer))
                        .map(|&peer| Input::Backlog { peer, pending: 0 }),
                );
                self.queued = pending.keys().copied().collect();
                inputs
            }
        }
    }
}

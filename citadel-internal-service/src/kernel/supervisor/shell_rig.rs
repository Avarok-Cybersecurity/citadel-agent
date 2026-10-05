//! Doubles for the shell: a manual clock, scripted answers, and a log of every call in the
//! order the shell made it.

use super::core::Core;
use super::policy::{Backoff, SupervisorPolicy};
use super::ports::{
    Backlog, Clock, NetworkWatch, PathError, Paths, PeerDialer, Reporter, ServerLink,
};
use super::shell::{run, Ports, Signal};
use super::types::{Cid, DialOutcome, Millis, ProbeOutcome, SupervisorEvent};
use futures::future::BoxFuture;
use futures::FutureExt;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

pub(super) const PEER: Cid = 11;
/// A peer only for `Rig::sync`.
pub(super) const SYNC: Cid = 999;

/// Fixed numbers, not the agent's; jitter off so a backoff is exact.
pub(super) const POLICY: SupervisorPolicy = SupervisorPolicy {
    probe_interval: Duration::from_secs(86_400),
    probe_timeout: Duration::from_secs(5),
    missed_probes: 2,
    dial_backoff: Backoff {
        initial: Duration::from_secs(1),
        max: Duration::from_secs(30),
    },
    upgrade_backoff: Backoff {
        initial: Duration::from_secs(5),
        max: Duration::from_secs(300),
    },
    stable_after: Duration::from_secs(60),
    redial_window: Duration::from_secs(600),
    interest_ceiling: Duration::from_secs(120),
    backlog_poll: Duration::from_secs(2),
    jitter_seed: 7,
    jitter_permille: 0,
    dial_peers: true,
};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Call {
    Probe,
    Force,
    Rebind,
    Dial(Cid),
    Upgrade(Cid, bool),
    Report(SupervisorEvent),
}

pub(super) struct ManualClock(watch::Sender<u64>);

impl Clock for ManualClock {
    fn now(&self) -> Millis {
        Millis(*self.0.borrow())
    }

    fn sleep_until(&self, at: Millis) -> BoxFuture<'static, ()> {
        let mut rx = self.0.subscribe();
        async move {
            while *rx.borrow_and_update() < at.0 {
                if rx.changed().await.is_err() {
                    return;
                }
            }
        }
        .boxed()
    }
}

pub(super) struct Doubles {
    log: mpsc::UnboundedSender<Call>,
    pub(super) probes: Mutex<VecDeque<ProbeOutcome>>,
    dials: Mutex<VecDeque<DialOutcome>>,
    pub(super) backlog: Mutex<HashMap<Cid, u32>>,
    pub(super) force_ends_link: AtomicBool,
    /// When set, the next probe answers only when this resolves.
    pub(super) probe_gate: Mutex<Option<tokio::sync::oneshot::Receiver<ProbeOutcome>>>,
}

impl ServerLink for Doubles {
    fn probe(&self, _timeout: Duration) -> BoxFuture<'static, ProbeOutcome> {
        let _ = self.log.send(Call::Probe);
        if let Some(gate) = self.probe_gate.lock().take() {
            return async move { gate.await.expect("the test answers the probe") }.boxed();
        }
        let outcome = self.probes.lock().pop_front().unwrap_or(ProbeOutcome::Ok {
            rtt: Duration::from_millis(1),
        });
        async move { outcome }.boxed()
    }

    fn force_reconnect(&self) -> BoxFuture<'static, Result<(), String>> {
        let _ = self.log.send(Call::Force);
        let ended = self.force_ends_link.load(Ordering::SeqCst);
        async move { ended.then_some(()).ok_or_else(|| "no".to_string()) }.boxed()
    }
}

impl PeerDialer for Doubles {
    fn dial(&self, peer: Cid) -> BoxFuture<'static, DialOutcome> {
        let _ = self.log.send(Call::Dial(peer));
        let outcome = self.dials.lock().pop_front().unwrap_or(DialOutcome::Failed);
        async move { outcome }.boxed()
    }
}

impl Paths for Doubles {
    fn upgrade(&self, peer: Cid, restore_udp: bool) -> BoxFuture<'static, Result<(), PathError>> {
        let _ = self.log.send(Call::Upgrade(peer, restore_udp));
        async { Ok(()) }.boxed()
    }

    fn rebind(&self) -> BoxFuture<'static, Result<(), PathError>> {
        let _ = self.log.send(Call::Rebind);
        async { Ok(()) }.boxed()
    }
}

impl Backlog for Doubles {
    fn pending(&self) -> BoxFuture<'static, Result<HashMap<Cid, u32>, String>> {
        let pending = self.backlog.lock().clone();
        async move { Ok(pending) }.boxed()
    }
}

impl Reporter for Doubles {
    fn report(&self, event: SupervisorEvent) {
        let _ = self.log.send(Call::Report(event));
    }
}

struct ChannelWatch(mpsc::UnboundedReceiver<()>);

impl NetworkWatch for ChannelWatch {
    fn next_change(&mut self) -> BoxFuture<'_, Option<()>> {
        self.0.recv().boxed()
    }
}

pub(super) struct Rig {
    pub(super) clock: Arc<ManualClock>,
    pub(super) calls: mpsc::UnboundedReceiver<Call>,
    pub(super) signals: mpsc::UnboundedSender<Signal>,
    pub(super) network: mpsc::UnboundedSender<()>,
    pub(super) doubles: Arc<Doubles>,
    pub(super) task: tokio::task::JoinHandle<()>,
}

pub(super) fn rig(policy: SupervisorPolicy) -> Rig {
    let (log, calls) = mpsc::unbounded_channel();
    let doubles = Arc::new(Doubles {
        log,
        probes: Mutex::default(),
        dials: Mutex::default(),
        backlog: Mutex::default(),
        force_ends_link: AtomicBool::new(true),
        probe_gate: Mutex::default(),
    });
    let clock = Arc::new(ManualClock(watch::channel(0).0));
    let (signals, signals_rx) = mpsc::unbounded_channel();
    let (network, network_rx) = mpsc::unbounded_channel();
    let ports = Ports {
        clock: clock.clone(),
        link: doubles.clone(),
        dialer: doubles.clone(),
        paths: doubles.clone(),
        backlog: doubles.clone(),
        reporter: doubles.clone(),
    };
    let task = tokio::spawn(run(
        Core::new(policy, 1),
        policy,
        Box::new(ChannelWatch(network_rx)),
        ports,
        signals_rx,
    ));
    Rig {
        clock,
        calls,
        signals,
        network,
        doubles,
        task,
    }
}

impl Rig {
    /// The next call. The bound only turns a hang into a failure; nothing waits on it.
    pub(super) async fn next(&mut self) -> Call {
        tokio::time::timeout(Duration::from_secs(10), self.calls.recv())
            .await
            .expect("the shell made no further call")
            .expect("the shell stopped")
    }

    pub(super) fn advance(&self, millis: u64) {
        self.clock.0.send_modify(|now| *now += millis);
    }

    /// Returns once the shell has handled every signal sent before it: signals are handled in
    /// order, and this one's three calls are made when it is. Its interest lapses at once.
    pub(super) async fn sync(&mut self) {
        self.signals
            .send(Signal::Interest {
                peer: SYNC,
                ttl: Duration::from_millis(1),
            })
            .unwrap();
        assert_eq!(
            self.next().await,
            Call::Report(SupervisorEvent::PeerDialing { peer_cid: SYNC })
        );
        assert_eq!(self.next().await, Call::Dial(SYNC));
        assert_eq!(
            self.next().await,
            Call::Report(SupervisorEvent::Degraded { peer_cid: SYNC })
        );
    }

    /// A network change: its two calls are a marker nothing else in these tests produces.
    pub(super) async fn marker(&mut self) {
        self.network.send(()).expect("the shell is running");
        assert_eq!(self.next().await, Call::Rebind);
        assert_eq!(self.next().await, Call::Probe);
    }
}

//! Which accounts have a supervisor, and the one place one is started.

use super::core::Core;
use super::policy::SupervisorPolicy;
use super::ports::NetworkWatch;
use super::shell::{self, Ports, Signal};
use citadel_sdk::prelude::PathControl;
use parking_lot::Mutex;
use std::collections::HashMap;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::task::JoinHandle;

/// A running supervisor: stopping it is dropping it.
struct Slot {
    signals: UnboundedSender<Signal>,
    task: JoinHandle<()>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// At most one supervisor per CID, like the ILM it runs beside.
#[derive(Default)]
pub(crate) struct Supervisors {
    slots: Mutex<HashMap<u64, Slot>>,
    /// The SDK's handle for re-arming each live peer connection's path campaign. It outlives
    /// the channel's `split`, so it is kept here, where the supervisor's `Paths` finds it.
    controls: Mutex<HashMap<(u64, u64), PathControl>>,
}

impl Supervisors {
    /// Start `cid`'s supervisor unless it is running. Returns whether this call started it,
    /// so the caller seeds it with what is already true of the account.
    pub(crate) fn start(
        &self,
        cid: u64,
        policy: SupervisorPolicy,
        network: Box<dyn NetworkWatch>,
        ports: Ports,
    ) -> bool {
        let mut slots = self.slots.lock();
        if slots.contains_key(&cid) {
            return false;
        }
        let (signals, rx) = unbounded_channel();
        let task = tokio::spawn(shell::run(
            Core::new(policy, cid),
            policy,
            network,
            ports,
            rx,
        ));
        slots.insert(cid, Slot { signals, task });
        true
    }

    /// Tell `cid`'s supervisor, if it has one.
    pub(crate) fn signal(&self, cid: u64, signal: Signal) {
        if let Signal::PeerLost(peer) | Signal::PeerReleased(peer) = signal {
            self.controls.lock().remove(&(cid, peer));
        }
        if let Some(slot) = self.slots.lock().get(&cid) {
            // A closed channel means the supervisor already ended with its session.
            let _ = slot.signals.send(signal);
        }
    }

    pub(crate) fn is_running(&self, cid: u64) -> bool {
        self.slots.lock().contains_key(&cid)
    }

    /// A peer connection came up: its path campaign can be re-armed through `control`.
    pub(crate) fn set_path_control(&self, cid: u64, peer: u64, control: PathControl) {
        self.controls.lock().insert((cid, peer), control);
    }

    pub(crate) fn path_control(&self, cid: u64, peer: u64) -> Option<PathControl> {
        self.controls.lock().get(&(cid, peer)).cloned()
    }

    /// The session ended.
    pub(crate) fn stop(&self, cid: u64) {
        self.slots.lock().remove(&cid);
        self.controls
            .lock()
            .retain(|(session, _), _| *session != cid);
    }
}

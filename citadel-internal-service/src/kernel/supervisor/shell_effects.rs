//! Doing what the core's commands say: each one reaches its port, and what comes back
//! reaches the loop as a `Done`.

use super::ports::PathError;
use super::shell::{Done, Shell, LOG};
use super::types::{Command, Input};
use citadel_sdk::logging::{info, warn};

impl Shell {
    pub(super) fn read_backlog(&mut self) {
        let read = self.ports.backlog.pending();
        let done = self.done.clone();
        self.effects.spawn(async move {
            match read.await {
                Ok(pending) => {
                    let _ = done.send(Done::Backlog(pending));
                }
                Err(reason) => warn!(target: LOG, "the ILM queue could not be read: {reason}"),
            }
        });
    }

    pub(super) fn apply(&mut self, input: Input) {
        for command in self.core.step(input) {
            self.execute(command);
        }
    }

    fn execute(&mut self, command: Command) {
        let done = self.done.clone();
        match command {
            Command::ProbeServer => {
                let probe = self.ports.link.probe(self.policy.probe_timeout);
                let clock = self.ports.clock.clone();
                let handle = self.effects.spawn(async move {
                    let outcome = probe.await;
                    let _ = done.send(Done::Probe(clock.now(), outcome));
                });
                self.probe = Some(handle);
            }
            Command::ForceReconnect => {
                info!(target: LOG, "the server stopped answering; ending the link to reconnect it");
                let force = self.ports.link.force_reconnect();
                let clock = self.ports.clock.clone();
                self.effects.spawn(async move {
                    if let Err(reason) = force.await {
                        warn!(target: LOG, "the link could not be ended: {reason}");
                        let _ = done.send(Done::ForceFailed(clock.now()));
                    }
                });
            }
            Command::RebindTransports => {
                let rebind = self.ports.paths.rebind();
                self.path_effect("rebind_local", rebind);
            }
            Command::DialPeer { peer } => {
                let dial = self.ports.dialer.dial(peer);
                let clock = self.ports.clock.clone();
                self.effects.spawn(async move {
                    let outcome = dial.await;
                    let _ = done.send(Done::Dial(clock.now(), peer, outcome));
                });
            }
            Command::UpgradePath { peer, restore_udp } => {
                let upgrade = self.ports.paths.upgrade(peer, restore_udp);
                self.path_effect("upgrade", upgrade);
            }
            Command::Report(event) => self.ports.reporter.report(event),
        }
    }

    fn path_effect(
        &mut self,
        what: &'static str,
        call: futures::future::BoxFuture<'static, Result<(), PathError>>,
    ) {
        let say = !self.said_refused.contains(&what);
        if say {
            self.said_refused.push(what);
        }
        self.effects.spawn(async move {
            match call.await {
                Ok(()) => {}
                Err(PathError::Refused(why)) if say => warn!(target: LOG, "{what} refused: {why}"),
                Err(PathError::Refused(_)) => {}
                Err(PathError::Failed(reason)) => warn!(target: LOG, "{what} failed: {reason}"),
            }
        });
    }
}

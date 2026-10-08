//! The production ends of the supervisor's ports, and starting a supervisor for an account.

mod clock;
mod dialer;
mod network;
mod paths;
mod reporter;
mod server_link;

use super::ports::{Backlog, NetworkWatch};
use super::shell::{Ports, Signal};
use super::types::{Cid, LinkStatus};
use crate::kernel::ilm::{pending_by_peer, HostIo};
use crate::kernel::reconnect::LinkState;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::Ratchet;
use futures::future::BoxFuture;
use futures::FutureExt;
use std::collections::HashMap;
use std::sync::Arc;

struct IlmBacklog<T, R: Ratchet> {
    this: CitadelWorkspaceService<T, R>,
    cid: u64,
}

impl<T: IOInterface + Sync, R: Ratchet> Backlog for IlmBacklog<T, R> {
    fn pending(&self) -> BoxFuture<'static, Result<HashMap<Cid, u32>, String>> {
        let io: Arc<dyn HostIo> = Arc::new(self.this.clone());
        pending_by_peer(self.cid, io).boxed()
    }
}

/// With no network watch the supervisor still probes and dials; it only misses the OS's
/// word of an address change.
struct NoWatch;

impl NetworkWatch for NoWatch {
    fn next_change(&mut self) -> BoxFuture<'_, Option<()>> {
        async { None }.boxed()
    }
}

impl<T: IOInterface + Sync, R: Ratchet> CitadelWorkspaceService<T, R> {
    /// Start `cid`'s supervisor, beside its ILM, if this service supervises and it has none.
    pub(crate) fn supervise(&self, cid: u64) {
        let Some(policy) = self.supervisor else {
            return;
        };
        let started = self.supervisors.start(
            cid,
            policy,
            self.network_watch(cid),
            Ports {
                clock: Arc::new(clock::TokioClock::new()),
                link: Arc::new(server_link::KernelLink {
                    this: self.clone(),
                    cid,
                    probed: Arc::default(),
                }),
                dialer: Arc::new(dialer::KernelDialer {
                    this: self.clone(),
                    cid,
                }),
                paths: Arc::new(paths::SdkPaths {
                    this: self.clone(),
                    supervisors: self.supervisors.clone(),
                    cid,
                }),
                backlog: Arc::new(IlmBacklog {
                    this: self.clone(),
                    cid,
                }),
                reporter: Arc::new(reporter::WindowReporter {
                    this: self.clone(),
                    cid,
                }),
            },
        );
        if started {
            self.seed_supervisor(cid);
        }
    }

    fn network_watch(&self, cid: u64) -> Box<dyn NetworkWatch> {
        match network::IfWatch::new() {
            Ok(watch) => Box::new(watch),
            Err(err) => {
                warn!(target: "citadel::supervisor", "{cid}: no network watch, address changes will not be noticed: {err}");
                Box::new(NoWatch)
            }
        }
    }

    /// What was already true when the supervisor started: a link that is mid-reconnect.
    fn seed_supervisor(&self, cid: u64) {
        let link = self
            .server_connection_map
            .read()
            .get(&cid)
            .map(|conn| conn.link);
        if matches!(link, Some(LinkState::Reconnecting)) {
            self.supervisors
                .signal(cid, Signal::Link(LinkStatus::Reconnecting));
        }
    }
}

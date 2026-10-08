//! `ServerLink` over the account's SDK session.

use crate::kernel::reconnect::instance::{self, Instance};
use crate::kernel::reconnect::{force, LinkState};
use crate::kernel::supervisor::ports::ServerLink;
use crate::kernel::supervisor::types::ProbeOutcome;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::logging::info;
use citadel_sdk::prelude::{ProtocolRemoteExt, Ratchet, ServerProbeOutcome};
use futures::future::BoxFuture;
use futures::FutureExt;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;

pub(super) struct KernelLink<T, R: Ratchet> {
    pub this: CitadelWorkspaceService<T, R>,
    pub cid: u64,
    /// The link the latest probe was sent on: the one a force is about.
    pub probed: Arc<Mutex<Option<Instance>>>,
}

impl<T: IOInterface + Sync, R: Ratchet> ServerLink for KernelLink<T, R> {
    /// The SDK's probe: one authenticated round trip now, not on the 15 minute keep-alive
    /// schedule. A server below protocol 0.12.1 cannot answer one, which the SDK reports as
    /// an error, and an error says nothing about the path: such a link is not probed.
    fn probe(&self, timeout: Duration) -> BoxFuture<'static, ProbeOutcome> {
        let (this, cid, probed) = (self.this.clone(), self.cid, self.probed.clone());
        async move {
            let up = this
                .server_connection_map
                .read()
                .get(&cid)
                .filter(|conn| conn.link == LinkState::Up)
                .map(|conn| conn.instance);
            let Some(instance) = up else {
                return ProbeOutcome::Error;
            };
            *probed.lock() = Some(instance);
            match this.remote().probe_server(cid, timeout).await {
                ServerProbeOutcome::Ok(rtt) => ProbeOutcome::Ok { rtt },
                ServerProbeOutcome::Timeout => ProbeOutcome::Timeout,
                ServerProbeOutcome::Error(err) => {
                    info!(target: "citadel::supervisor", "{cid}: the probe could not be made: {err:?}");
                    ProbeOutcome::Error
                }
            }
        }
        .boxed()
    }

    /// Abandons the SDK session locally, with no server ack, then runs the reconnect a
    /// dropped link gets: it signs in again with the stored credentials, keeps the CID, and
    /// the resume token replaces the server's stale copy. `Err` when the SDK holds no such
    /// session, or when the link is no longer the one the probes went unanswered on: the SDK
    /// abandons by CID, and the link that replaced it has not been found dead.
    fn force_reconnect(&self) -> BoxFuture<'static, Result<(), String>> {
        let (this, cid, probed) = (self.this.clone(), self.cid, *self.probed.lock());
        async move {
            let probed = probed.ok_or("no probe named a link to end")?;
            let gate = this
                .server_connection_map
                .read()
                .get(&cid)
                .map(|conn| conn.handoff.clone())
                .ok_or("the session is gone")?;
            // No reconnect attempt can install a new link while this is held, so the link
            // checked is the one abandoned.
            let attempts = gate.hold_attempts().await;
            let current = this
                .server_connection_map
                .read()
                .get(&cid)
                .map(|conn| (conn.link, conn.instance));
            if !instance::probed_link_is_current(current, probed) {
                return Err(format!(
                    "the link probed ({probed:?}) is no longer the session's ({current:?})"
                ));
            }
            this.remote()
                .abandon_session(cid)
                .await
                .map_err(|err| format!("{err:?}"))?;
            drop(attempts);
            force::restart(&this, cid);
            Ok(())
        }
        .boxed()
    }
}

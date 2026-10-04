//! `ServerLink` over the account's SDK session.

use crate::kernel::reconnect::LinkState;
use crate::kernel::supervisor::ports::ServerLink;
use crate::kernel::supervisor::types::ProbeOutcome;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::logging::info;
use citadel_sdk::prelude::{
    ProtocolRemoteExt, ProtocolRemoteTargetExt, Ratchet, ServerProbeOutcome,
};
use futures::future::BoxFuture;
use futures::FutureExt;
use std::time::Duration;

pub(super) struct KernelLink<T, R: Ratchet> {
    pub this: CitadelWorkspaceService<T, R>,
    pub cid: u64,
}

impl<T: IOInterface + Sync, R: Ratchet> ServerLink for KernelLink<T, R> {
    /// The SDK's probe: one authenticated round trip now, not on the 15 minute keep-alive
    /// schedule. A server below protocol 0.12.1 cannot answer one, which the SDK reports as
    /// an error, and an error says nothing about the path: such a link is not probed.
    fn probe(&self, timeout: Duration) -> BoxFuture<'static, ProbeOutcome> {
        let (this, cid) = (self.this.clone(), self.cid);
        async move {
            let up = this
                .server_connection_map
                .read()
                .get(&cid)
                .is_some_and(|conn| conn.link == LinkState::Up);
            if !up {
                return ProbeOutcome::Error;
            }
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

    /// Asks the SDK to disconnect the session, which it does only with the server's
    /// confirmation (up to 30 s). A confirmed disconnect is reported as the drop it is, and
    /// `kernel/reconnect` brings the session back as for any other. On a path that is
    /// truly dead nothing confirms it, so the link is left as it was: the SDK has no call
    /// that ends a session locally, and a reconnect started without one is refused
    /// ("Session ... already exists") until its give-up signs the account out.
    fn force_reconnect(&self) -> BoxFuture<'static, Result<(), String>> {
        let remote = self
            .this
            .server_connection_map
            .read()
            .get(&self.cid)
            .map(|conn| conn.client_server_remote.clone());
        async move {
            let remote = remote.ok_or("the session is gone")?;
            remote.disconnect().await.map_err(|err| format!("{err:?}"))
        }
        .boxed()
    }
}

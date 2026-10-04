//! `Paths` over the SDK: re-arming a peer connection's path campaign, and rebinding the
//! node's QUIC endpoints to the current local address.

use crate::kernel::supervisor::ports::{PathError, Paths};
use crate::kernel::supervisor::types::Cid;
use crate::kernel::supervisor::Supervisors;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::logging::info;
use citadel_sdk::prelude::Ratchet;
use futures::future::BoxFuture;
use futures::FutureExt;
use std::sync::Arc;

pub(super) struct SdkPaths<T, R: Ratchet> {
    pub this: CitadelWorkspaceService<T, R>,
    pub supervisors: Arc<Supervisors>,
    pub cid: u64,
}

impl<T: IOInterface + Sync, R: Ratchet> Paths for SdkPaths<T, R> {
    /// The SDK refuses it for a peer below protocol 0.12.1, and once the campaign has ended
    /// or the connection closed; those are refusals, not failures.
    fn upgrade(&self, peer: Cid, restore_udp: bool) -> BoxFuture<'static, Result<(), PathError>> {
        let control = self.supervisors.path_control(self.cid, peer);
        async move {
            let control = control.ok_or_else(|| PathError::Refused("no live connection".into()))?;
            control
                .upgrade(restore_udp)
                .map_err(|err| PathError::Refused(format!("{err:?}")))
        }
        .boxed()
    }

    fn rebind(&self) -> BoxFuture<'static, Result<(), PathError>> {
        let (this, cid) = (self.this.clone(), self.cid);
        async move {
            let report = this
                .remote()
                .rebind_local()
                .map_err(|err| PathError::Failed(format!("{err:?}")))?;
            info!(target: "citadel::supervisor", "{cid}: rebound {} QUIC endpoint(s), {} failed", report.rebound.len(), report.failed.len());
            match report.failed.first() {
                Some(failure) => Err(PathError::Failed(failure.error.clone())),
                None => Ok(()),
            }
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use citadel_internal_service_connector::io_interface::in_memory::InMemoryInterface;
    use citadel_sdk::prelude::StackedRatchet;

    type Svc = CitadelWorkspaceService<InMemoryInterface, StackedRatchet>;

    #[tokio::test]
    async fn re_arming_a_peer_with_no_live_connection_is_refused_not_ignored() {
        let (_connector, this): (_, Svc) =
            CitadelWorkspaceService::new_in_memory(crate::SERVER_RECONNECT);
        let paths = SdkPaths {
            supervisors: this.supervisors.clone(),
            this,
            cid: 1,
        };
        let refused = paths.upgrade(2, true).await;
        assert!(matches!(refused, Err(PathError::Refused(_))), "{refused:?}");
    }
}

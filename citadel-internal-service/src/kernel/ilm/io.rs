//! The agent's side of [`HostIo`].

use super::HostIo;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, MessageNotification,
};
use citadel_sdk::prelude::Ratchet;
use futures::future::BoxFuture;
use uuid::Uuid;

impl<T, R> HostIo for CitadelWorkspaceService<T, R>
where
    T: IOInterface + Sync,
    R: Ratchet,
{
    fn local_db(
        &self,
        request: InternalServiceRequest,
    ) -> BoxFuture<'static, InternalServiceResponse> {
        let this = self.clone();
        Box::pin(async move { crate::kernel::requests::answer_local_db(&this, request).await })
    }

    fn send_frame(
        &self,
        request: InternalServiceRequest,
    ) -> BoxFuture<'static, Result<(), String>> {
        let this = self.clone();
        Box::pin(async move {
            // The same path a browser's `Message` takes, minus the gate: this is
            // the account's own ILM sending as the account.
            match crate::kernel::requests::send_message(&this, Uuid::nil(), request).await {
                Some(InternalServiceResponse::MessageSendSuccess(_)) => Ok(()),
                Some(InternalServiceResponse::MessageSendFailure(failure)) => Err(failure.message),
                other => Err(format!("unexpected answer to an ILM frame: {other:?}")),
            }
        })
    }

    fn connected_peers(&self, cid: u64) -> Vec<u64> {
        self.server_connection_map
            .read()
            .get(&cid)
            .map(|conn| conn.peers.keys().copied().collect())
            .unwrap_or_default()
    }

    /// A hosted account's delivered messages go to its conversation store,
    /// which stores, acknowledges and announces them -- with or without a
    /// window open (kernel/conversations).
    fn deliver(&self, cid: u64, notification: MessageNotification) -> BoxFuture<'static, bool> {
        let this = self.clone();
        Box::pin(async move {
            debug_assert_eq!(notification.cid, cid);
            this.conversations.delivered(&this, notification).await
        })
    }
}

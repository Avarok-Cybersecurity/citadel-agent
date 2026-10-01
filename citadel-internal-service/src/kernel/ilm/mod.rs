//! One ILM per signed-in account, hosted by the agent.
//!
//! ILM must run in exactly one place per CID: the receiver's per-source delivery
//! frontier treats a second ILM's lower id sequence as duplicates, and each ILM
//! rewrites its whole tracker map. Hosting it here, rather than in a browser
//! window, is what lets every window of an account be an equal subscriber, and
//! lets the account receive with no window open at all
//! (citadel-workspace `docs/plans/multi-window-sessions.md`, mw3).
//!
//! The pieces are the browser's, over the agent's own I/O:
//! * storage: the connector's `CitadelWorkspaceBackend` over [`AgentChannel`],
//!   which answers its LocalDB requests in-process -- the same keys a browser's
//!   ILM persisted, so that state is adopted, not reset;
//! * wire: the connector's `wire` module, both ways, so frames are byte-identical
//!   to the ones a browser sends and a peer on an older agent reads them;
//! * transport: the session's peer sinks ([`AgentTransport`]);
//! * delivery: [`AgentDelivery`], to the windows attached to the session.
//!
//! The agent's own effects are reached through [`HostIo`], so everything here is
//! tested against a double with no SDK and no sockets.

mod channel;
mod delivery;
mod io;
mod registry;
mod transport;

#[cfg(test)]
mod tests;

pub(crate) use registry::IlmRegistry;

use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, MessageNotification,
};
use futures::future::BoxFuture;

/// What an account's ILM needs from the agent around it.
pub(crate) trait HostIo: Send + Sync + 'static {
    /// Answer a LocalDB (or Batched) request for the account's own store.
    fn local_db(
        &self,
        request: InternalServiceRequest,
    ) -> BoxFuture<'static, InternalServiceResponse>;
    /// Put an encoded frame (`InternalServiceRequest::Message`) on the peer link.
    fn send_frame(&self, request: InternalServiceRequest)
        -> BoxFuture<'static, Result<(), String>>;
    /// The peers this account has a live P2P link to right now.
    fn connected_peers(&self, cid: u64) -> Vec<u64>;
    /// Hand a delivered message on. `false` means nobody took it, and ILM keeps
    /// it to deliver again.
    fn deliver(&self, cid: u64, notification: MessageNotification) -> BoxFuture<'static, bool>;
}

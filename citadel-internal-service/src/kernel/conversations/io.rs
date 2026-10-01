//! What the conversation store needs from the agent around it (SBIO): its
//! records, the windows, the peer link, a clock and fresh ids. The agent's
//! implementation is in kernel/ilm/io.rs; tests use a double.

use super::kv::ConversationKv;
use crate::kernel::notices::decide::NoticeSource;
use citadel_internal_service_types::InternalServiceResponse;
use futures::future::BoxFuture;

pub(crate) trait ConversationIo: Send + Sync {
    fn kv(&self) -> &dyn ConversationKv;
    /// Hand `response` to every window attached to `cid`; how many took it.
    fn publish(&self, cid: u64, response: InternalServiceResponse) -> usize;
    /// Send a P2P command to `peer` through the account's ILM. `Ok` once ILM has it.
    fn send_p2p(
        &self,
        cid: u64,
        peer: u64,
        bytes: Vec<u8>,
    ) -> BoxFuture<'static, Result<(), String>>;
    fn now_ms(&self) -> f64;
    fn new_id(&self) -> String;
    fn account_username(&self, cid: u64) -> String;
    /// The peer's username, if the agent has heard it (registration, peer list).
    fn peer_username(&self, cid: u64, peer: u64) -> Option<String>;
    /// Is `peer` someone this account has registered or is connected to?
    fn knows_peer(&self, cid: u64, peer: u64) -> BoxFuture<'static, bool>;
    /// Something happened the user may need telling about (kernel/notices).
    fn raise_notice(&self, cid: u64, source: NoticeSource);
    /// A conversation changed: the account rows' unread counts may have.
    fn rows_changed(&self);
}

//! What the supervisor needs from the world. The core knows none of it; the shell calls
//! these and feeds the answers back as inputs. Production ends are in `adapters`.

use super::types::{Cid, DialOutcome, Millis, ProbeOutcome, SupervisorEvent};
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::time::Duration;

pub(crate) trait Clock: Send + Sync + 'static {
    fn now(&self) -> Millis;
    /// Resolves once the clock reaches `at`.
    fn sleep_until(&self, at: Millis) -> BoxFuture<'static, ()>;
}

/// A stream of "the set of interfaces or addresses changed".
pub(crate) trait NetworkWatch: Send + 'static {
    /// `None` when the watch has ended and no change will ever come.
    fn next_change(&mut self) -> BoxFuture<'_, Option<()>>;
}

pub(crate) trait ServerLink: Send + Sync + 'static {
    /// One authenticated round trip to the server, bounded by `timeout`.
    fn probe(&self, timeout: Duration) -> BoxFuture<'static, ProbeOutcome>;
    /// End the link now, so `kernel/reconnect` brings it back as it does for any drop.
    /// `Err` says why it could not be ended, and leaves it as it was.
    fn force_reconnect(&self) -> BoxFuture<'static, Result<(), String>>;
}

pub(crate) trait PeerDialer: Send + Sync + 'static {
    fn dial(&self, peer: Cid) -> BoxFuture<'static, DialOutcome>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PathError {
    /// The call is refused for a reason that will not change by asking again at once: the
    /// peer is on an older protocol, or the connection is gone.
    Refused(String),
    Failed(String),
}

pub(crate) trait Paths: Send + Sync + 'static {
    /// Re-arm the SDK's path campaign for `peer`.
    fn upgrade(&self, peer: Cid, restore_udp: bool) -> BoxFuture<'static, Result<(), PathError>>;
    /// Rebind every live QUIC endpoint to the current local address.
    fn rebind(&self) -> BoxFuture<'static, Result<(), PathError>>;
}

/// Messages queued for each peer in the account's ILM.
pub(crate) trait Backlog: Send + Sync + 'static {
    fn pending(&self) -> BoxFuture<'static, Result<HashMap<Cid, u32>, String>>;
}

/// Tells the account's windows.
pub(crate) trait Reporter: Send + Sync + 'static {
    fn report(&self, event: SupervisorEvent);
}

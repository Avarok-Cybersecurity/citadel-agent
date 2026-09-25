//! Node events that can arrive before the session they belong to is in the map.
//!
//! A server that still lists a member in a group prompts it to rejoin as soon as the
//! member's connect is acknowledged, and the SDK then delivers an unsolicited
//! `GroupChannelCreated` (and possibly `GroupEvent`s) for that session. `connect.rs`
//! inserts the `Connection` only after several further awaits (username, server
//! address, host, credential fingerprint), so the event can win that race.
//!
//! Handling it against an empty map is not a harmless miss: the channel is dropped,
//! and dropping a `GroupChannelRecvHalf` sends `LeaveRoom` over the live session, which
//! removes the member from the group for real. So such an event waits, bounded, for its
//! session to be mapped instead of being handled against nothing.

use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{NetworkError, Ratchet};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

/// How long an event waits for its session. Connect's remaining steps are local reads
/// and one key derivation; this is far past them, and short enough that an event for a
/// session that never appears (a connect refused after the SDK accepted it) is let go.
pub(crate) const SESSION_MAPPED_WITHIN: Duration = Duration::from_secs(10);

const POLL_EVERY: Duration = Duration::from_millis(20);

fn is_mapped<V>(map: &Arc<RwLock<HashMap<u64, V>>>, cid: u64) -> bool {
    map.read().contains_key(&cid)
}

/// Whether `cid` is in the map now or becomes so within `within`.
pub(crate) async fn mapped_within<V>(
    map: &Arc<RwLock<HashMap<u64, V>>>,
    cid: u64,
    within: Duration,
) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if is_mapped(map, cid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL_EVERY).await;
    }
}

/// Runs `handle` now if `cid` is mapped; otherwise in a task, once it is. The node-event
/// loop is not held up while it waits.
pub(crate) async fn once_session_mapped<T, R, F, Fut>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    what: &'static str,
    handle: F,
) -> Result<(), NetworkError>
where
    T: IOInterface + Sync,
    R: Ratchet,
    F: FnOnce(CitadelWorkspaceService<T, R>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), NetworkError>> + Send + 'static,
{
    if is_mapped(&this.server_connection_map, cid) {
        return handle(this.clone()).await;
    }
    info!(target: "citadel", "[{what}] session {cid} is not mapped yet; waiting up to {SESSION_MAPPED_WITHIN:?}");
    let this = this.clone();
    drop(tokio::spawn(async move {
        if !mapped_within(&this.server_connection_map, cid, SESSION_MAPPED_WITHIN).await {
            warn!(target: "citadel", "[{what}] session {cid} was not mapped within {SESSION_MAPPED_WITHIN:?}; letting the event go");
            return;
        }
        if let Err(err) = handle(this).await {
            warn!(target: "citadel", "[{what}] for session {cid} failed after it was mapped: {err:?}");
        }
    }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_map() -> Arc<RwLock<HashMap<u64, ()>>> {
        Arc::new(RwLock::new(HashMap::new()))
    }

    #[tokio::test]
    async fn an_event_waits_for_a_session_that_is_mapped_late() {
        let map = empty_map();
        let inserter = map.clone();
        drop(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = inserter.write().insert(7, ());
        }));
        assert!(mapped_within(&map, 7, Duration::from_secs(5)).await);
    }

    #[tokio::test]
    async fn an_event_for_a_session_that_never_appears_is_let_go() {
        let map = empty_map();
        let _ = map.write().insert(8, ());
        assert!(!mapped_within(&map, 7, Duration::from_millis(100)).await);
    }
}

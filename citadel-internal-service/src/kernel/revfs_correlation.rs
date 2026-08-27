//! Correlates REVFS transfer ticks with the localhost request that caused them.
//!
//! The SDK's `PullObject` / `SendObject` carry no application request id, so
//! when the resulting `ObjectTransferHandle` arrives the kernel has nothing to
//! stamp the `FileTransferTickNotification`s with and used to fall back to the
//! TCP-connection uuid (`request_id.unwrap_or(uuid)` in `spawn_tick_updater`).
//! The browser correlates ticks by the `request_id` it sent in `DownloadFile`
//! or `SendFile`, so that fallback matched nothing: every REVFS download
//! completed on disk and then reported failure after the 30s timeout, and
//! every REVFS upload had no completion signal at all.
//!
//! This registry is written by the `DownloadFile` / `SendFile` request
//! handlers and consumed by `responses/object_transfer_handle.rs` when the
//! matching handle arrives, restoring the browser's request id to the tick
//! stream.
//!
//! Correlation is FIFO per (direction, scope key): the SDK offers no stronger
//! join key — the puller does not know the `object_id` in advance, and the
//! pull response's metadata carries `TransferType::FileTransfer`, not the
//! virtual path. Requests to the same scope travel one ordered stream, so
//! FIFO matches whenever the remote answers in order; a mismatch under racing
//! same-scope transfers degrades to the pre-fix behaviour (timeout), never to
//! settling a different session's wait.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// How long a registered correlation may wait for its `ObjectTransferHandle`.
///
/// A pull whose remote side errors never produces a handle (the kernel has no
/// ReVFS-error `NodeResult` handler), so its entry would otherwise sit at the
/// head of the FIFO forever and misattribute every later transfer in that
/// scope. Chosen LONGER than the browser's 30s operation timeout so a live
/// wait is never expired, while a dead entry cannot shift the queue past the
/// next browser retry.
const CORRELATION_TTL: Duration = Duration::from_secs(60);

/// The scope key for client<->server transfers.
///
/// For a c2s REVFS pull the receiver-side handle carries
/// `source = C2S_IDENTITY_CID (0)`, so the handle-side key computation in
/// `responses/object_transfer_handle.rs` yields 0. Peer CIDs are never 0.
pub const SERVER_SCOPE: u64 = 0;

#[derive(Default)]
struct FifoByScope {
    queues: HashMap<u64, VecDeque<(Uuid, Instant)>>,
}

impl FifoByScope {
    fn register(&mut self, scope: u64, request_id: Uuid, now: Instant) {
        let queue = self.queues.entry(scope).or_default();
        // Prune entries whose waiter has long since timed out, so an
        // unanswered request cannot permanently desynchronise the FIFO.
        queue.retain(|(_, at)| now.duration_since(*at) < CORRELATION_TTL);
        queue.push_back((request_id, now));
    }

    fn take(&mut self, scope: u64, now: Instant) -> Option<Uuid> {
        let queue = self.queues.get_mut(&scope)?;
        queue.retain(|(_, at)| now.duration_since(*at) < CORRELATION_TTL);
        let id = queue.pop_front().map(|(id, _)| id);
        if queue.is_empty() {
            self.queues.remove(&scope);
        }
        id
    }

    /// Removes one specific registration — used when `remote.send` fails, so
    /// a request that never went out does not shift the FIFO for later ones.
    fn cancel(&mut self, scope: u64, request_id: Uuid) {
        if let Some(queue) = self.queues.get_mut(&scope) {
            queue.retain(|(id, _)| *id != request_id);
            if queue.is_empty() {
                self.queues.remove(&scope);
            }
        }
    }
}

/// Pending REVFS transfer correlations for one session (`Connection`).
///
/// Pulls (`DownloadFile` → Receiver handle) and pushes (`SendFile` → Sender
/// handle) are kept apart because a pull's ticks and a push's ticks arrive on
/// differently-oriented handles and must never consume each other's ids.
#[derive(Default)]
pub struct RevfsCorrelations {
    pulls: FifoByScope,
    pushes: FifoByScope,
}

impl RevfsCorrelations {
    pub fn register_pull(&mut self, scope: u64, request_id: Uuid) {
        self.register_pull_at(scope, request_id, Instant::now());
    }

    pub fn take_pull(&mut self, scope: u64) -> Option<Uuid> {
        self.take_pull_at(scope, Instant::now())
    }

    pub fn cancel_pull(&mut self, scope: u64, request_id: Uuid) {
        self.pulls.cancel(scope, request_id);
    }

    pub fn register_push(&mut self, scope: u64, request_id: Uuid) {
        self.register_push_at(scope, request_id, Instant::now());
    }

    pub fn take_push(&mut self, scope: u64) -> Option<Uuid> {
        self.take_push_at(scope, Instant::now())
    }

    pub fn cancel_push(&mut self, scope: u64, request_id: Uuid) {
        self.pushes.cancel(scope, request_id);
    }

    // Clock-injected variants so TTL behaviour is testable without sleeping.

    pub fn register_pull_at(&mut self, scope: u64, request_id: Uuid, now: Instant) {
        self.pulls.register(scope, request_id, now);
    }

    pub fn take_pull_at(&mut self, scope: u64, now: Instant) -> Option<Uuid> {
        self.pulls.take(scope, now)
    }

    pub fn register_push_at(&mut self, scope: u64, request_id: Uuid, now: Instant) {
        self.pushes.register(scope, request_id, now);
    }

    pub fn take_push_at(&mut self, scope: u64, now: Instant) -> Option<Uuid> {
        self.pushes.take(scope, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER: u64 = 42;

    #[test]
    fn pull_correlation_round_trips_in_fifo_order() {
        let mut c = RevfsCorrelations::default();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        c.register_pull(PEER, first);
        c.register_pull(PEER, second);
        assert_eq!(c.take_pull(PEER), Some(first));
        assert_eq!(c.take_pull(PEER), Some(second));
        assert_eq!(c.take_pull(PEER), None);
    }

    #[test]
    fn scopes_do_not_bleed_into_each_other() {
        let mut c = RevfsCorrelations::default();
        let for_peer = Uuid::new_v4();
        c.register_pull(PEER, for_peer);
        assert_eq!(c.take_pull(SERVER_SCOPE), None);
        assert_eq!(c.take_pull(PEER), Some(for_peer));
    }

    #[test]
    fn pushes_and_pulls_are_independent() {
        let mut c = RevfsCorrelations::default();
        let pull = Uuid::new_v4();
        let push = Uuid::new_v4();
        c.register_pull(PEER, pull);
        c.register_push(PEER, push);
        assert_eq!(c.take_push(PEER), Some(push));
        assert_eq!(c.take_pull(PEER), Some(pull));
    }

    #[test]
    fn a_dead_entry_expires_instead_of_shifting_the_fifo() {
        // A pull whose remote errored never produces a handle. Without the
        // TTL its id would sit at the queue head and the NEXT pull's handle
        // would be stamped with the dead browser wait's id — failing both.
        let mut c = RevfsCorrelations::default();
        let dead = Uuid::new_v4();
        let live = Uuid::new_v4();
        let t0 = Instant::now();
        c.register_pull_at(PEER, dead, t0);
        let later = t0 + CORRELATION_TTL + Duration::from_secs(1);
        c.register_pull_at(PEER, live, later);
        assert_eq!(c.take_pull_at(PEER, later), Some(live));
        assert_eq!(c.take_pull_at(PEER, later), None);
    }

    #[test]
    fn cancel_removes_only_the_failed_send() {
        let mut c = RevfsCorrelations::default();
        let failed = Uuid::new_v4();
        let ok = Uuid::new_v4();
        c.register_pull(PEER, failed);
        c.register_pull(PEER, ok);
        c.cancel_pull(PEER, failed);
        assert_eq!(c.take_pull(PEER), Some(ok));
        assert_eq!(c.take_pull(PEER), None);
    }
}

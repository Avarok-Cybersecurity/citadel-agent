//! Correlates REVFS pull ticks with the localhost request that caused them.
//! (Sends are joined by ticket instead: kernel/send_correlation.rs.)
//!
//! The SDK's `PullObject` carries no application request id, so
//! when the resulting `ObjectTransferHandle` arrives the kernel has nothing to
//! stamp the `FileTransferTickNotification`s with and used to fall back to the
//! TCP-connection uuid. The browser correlates ticks by the `request_id` it
//! sent in `DownloadFile`, so that fallback matched nothing: every REVFS
//! download completed on disk and then reported failure after the 30s timeout.
//!
//! This registry is written by the `DownloadFile` request handler and consumed by `responses/object_transfer_handle.rs` when the
//! matching handle arrives, restoring the browser's request id to the tick
//! stream.
//!
//! Correlation is FIFO per scope key: the SDK offers no stronger
//! join key — the puller does not know the `object_id` in advance, and the
//! pull response's metadata carries `TransferType::FileTransfer`, not the
//! virtual path. Requests to the same scope travel one ordered stream, so
//! FIFO matches whenever the remote answers in order; a mismatch under racing
//! same-scope transfers degrades to the pre-fix behaviour (timeout), never to
//! settling a different session's wait.

use crate::kernel::pulled_reads::PulledKey;
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

/// A registered pull: the browser's request id, and how the pull named its
/// file, so its saved output can be read back by that name (kernel/pulled_reads.rs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPull {
    pub request_id: Uuid,
    pub file: PulledKey,
}

#[derive(Default)]
struct FifoByScope {
    queues: HashMap<u64, VecDeque<(PendingPull, Instant)>>,
}

impl FifoByScope {
    fn register(&mut self, scope: u64, pull: PendingPull, now: Instant) {
        let queue = self.queues.entry(scope).or_default();
        // Prune entries whose waiter has long since timed out, so an
        // unanswered request cannot permanently desynchronise the FIFO.
        queue.retain(|(_, at)| now.duration_since(*at) < CORRELATION_TTL);
        queue.push_back((pull, now));
    }

    fn take(&mut self, scope: u64, now: Instant) -> Option<PendingPull> {
        let queue = self.queues.get_mut(&scope)?;
        queue.retain(|(_, at)| now.duration_since(*at) < CORRELATION_TTL);
        let id = queue.pop_front().map(|(pull, _)| pull);
        if queue.is_empty() {
            self.queues.remove(&scope);
        }
        id
    }

    /// Removes one specific registration — used when `remote.send` fails, so
    /// a request that never went out does not shift the FIFO for later ones.
    fn cancel(&mut self, scope: u64, request_id: Uuid) {
        if let Some(queue) = self.queues.get_mut(&scope) {
            queue.retain(|(pull, _)| pull.request_id != request_id);
            if queue.is_empty() {
                self.queues.remove(&scope);
            }
        }
    }
}

/// Pending REVFS pull correlations for one session (`Connection`).
///
/// Pushes are not here: a `SendFile`'s Sender handle is joined exactly, by
/// ticket, in kernel/send_correlation.rs. A pull's Receiver handle arrives
/// under the ticket of the byte-holder's answering send, so it cannot be.
#[derive(Default)]
pub struct RevfsCorrelations {
    pulls: FifoByScope,
}

impl RevfsCorrelations {
    pub fn register_pull(&mut self, scope: u64, pull: PendingPull) {
        self.register_pull_at(scope, pull, Instant::now());
    }

    pub fn take_pull(&mut self, scope: u64) -> Option<PendingPull> {
        self.take_pull_at(scope, Instant::now())
    }

    pub fn cancel_pull(&mut self, scope: u64, request_id: Uuid) {
        self.pulls.cancel(scope, request_id);
    }

    // Clock-injected variants so TTL behaviour is testable without sleeping.

    pub fn register_pull_at(&mut self, scope: u64, pull: PendingPull, now: Instant) {
        self.pulls.register(scope, pull, now);
    }

    pub fn take_pull_at(&mut self, scope: u64, now: Instant) -> Option<PendingPull> {
        self.pulls.take(scope, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER: u64 = 42;

    fn pull(request_id: Uuid) -> PendingPull {
        PendingPull {
            request_id,
            file: PulledKey {
                peer_cid: Some(PEER),
                virtual_directory: "/shared/a.txt".into(),
            },
        }
    }

    #[test]
    fn a_pull_keeps_how_it_named_its_file() {
        let mut c = RevfsCorrelations::default();
        let pending = pull(Uuid::new_v4());
        c.register_pull(PEER, pending.clone());
        assert_eq!(c.take_pull(PEER), Some(pending));
    }

    #[test]
    fn pull_correlation_round_trips_in_fifo_order() {
        let mut c = RevfsCorrelations::default();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        c.register_pull(PEER, pull(first));
        c.register_pull(PEER, pull(second));
        assert_eq!(c.take_pull(PEER).map(|p| p.request_id), Some(first));
        assert_eq!(c.take_pull(PEER).map(|p| p.request_id), Some(second));
        assert_eq!(c.take_pull(PEER).map(|p| p.request_id), None);
    }

    #[test]
    fn scopes_do_not_bleed_into_each_other() {
        let mut c = RevfsCorrelations::default();
        let for_peer = Uuid::new_v4();
        c.register_pull(PEER, pull(for_peer));
        assert_eq!(c.take_pull(SERVER_SCOPE).map(|p| p.request_id), None);
        assert_eq!(c.take_pull(PEER).map(|p| p.request_id), Some(for_peer));
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
        c.register_pull_at(PEER, pull(dead), t0);
        let later = t0 + CORRELATION_TTL + Duration::from_secs(1);
        c.register_pull_at(PEER, pull(live), later);
        assert_eq!(
            c.take_pull_at(PEER, later).map(|p| p.request_id),
            Some(live)
        );
        assert_eq!(c.take_pull_at(PEER, later).map(|p| p.request_id), None);
    }

    #[test]
    fn cancel_removes_only_the_failed_send() {
        let mut c = RevfsCorrelations::default();
        let failed = Uuid::new_v4();
        let ok = Uuid::new_v4();
        c.register_pull(PEER, pull(failed));
        c.register_pull(PEER, pull(ok));
        c.cancel_pull(PEER, failed);
        assert_eq!(c.take_pull(PEER).map(|p| p.request_id), Some(ok));
        assert_eq!(c.take_pull(PEER).map(|p| p.request_id), None);
    }
}

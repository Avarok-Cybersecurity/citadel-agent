//! Joins a `SendFile`'s Sender handle to the request that started it.
//!
//! `SendObject` carries no application request id, but the SDK stamps the
//! resulting `ObjectTransferHandle` with the ticket of the request that caused
//! it -- whichever path the handle takes to reach the kernel: the request's own
//! subscription, or the catch-all handler once the upload has stopped waiting
//! on that subscription (a person can take longer than `FIRST_EVENT_TIMEOUT` to
//! accept). Tickets are random 128-bit values, so the join is exact and does not
//! depend on handles arriving in send order.
//!
//! It replaces two weaker joins: a per-peer FIFO for RE-VFS pushes, and for a
//! chat transfer nothing at all -- its ticks carried the session's TCP uuid,
//! the one value every stream of the session shared.
//!
//! An entry lives until its handle arrives or the send is refused. A send that
//! is never answered keeps its entry for the life of the session, as the SDK
//! keeps its own state for that transfer.

use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

/// What a send left to be finished when its transfer ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSend {
    pub request_id: Uuid,
    /// The browser payload's request directory, for a `ByteContents` send: removed
    /// when the transfer ends (requests/file/browser_payload.rs).
    pub payload_dir: Option<PathBuf>,
}

#[derive(Default)]
pub struct SendCorrelations {
    by_ticket: HashMap<u128, PendingSend>,
}

impl SendCorrelations {
    pub fn register(&mut self, ticket: u128, send: PendingSend) {
        self.by_ticket.insert(ticket, send);
    }

    /// The send a Sender handle with `ticket` belongs to. `None` for a
    /// handle this session did not request -- e.g. this node answering a
    /// peer's RE-VFS pull, which arrives under the PEER's ticket.
    pub fn take(&mut self, ticket: u128) -> Option<PendingSend> {
        self.by_ticket.remove(&ticket)
    }
}

#[cfg(test)]
mod tests {
    use super::{PendingSend, SendCorrelations};
    use uuid::Uuid;

    fn send(request_id: Uuid) -> PendingSend {
        PendingSend {
            request_id,
            payload_dir: None,
        }
    }

    #[test]
    fn a_handle_finds_its_send_regardless_of_order() {
        let mut c = SendCorrelations::default();
        let (first, second) = (Uuid::new_v4(), Uuid::new_v4());
        c.register(1, send(first));
        c.register(2, send(second));
        assert_eq!(c.take(2), Some(send(second)));
        assert_eq!(c.take(1), Some(send(first)));
    }

    #[test]
    fn a_handle_is_joined_once() {
        let mut c = SendCorrelations::default();
        c.register(7, send(Uuid::new_v4()));
        assert!(c.take(7).is_some());
        assert_eq!(c.take(7), None);
    }

    #[test]
    fn an_unrequested_handle_names_nothing() {
        let mut c = SendCorrelations::default();
        c.register(7, send(Uuid::new_v4()));
        assert_eq!(c.take(8), None);
        assert!(
            c.take(7).is_some(),
            "an unrelated ticket must not consume an entry"
        );
    }
}

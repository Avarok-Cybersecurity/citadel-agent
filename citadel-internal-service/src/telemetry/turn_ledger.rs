//! What each session's peer connections moved through TURN since its last report to its server.
//!
//! Only the agent sees TURN traffic: the relay is a third party's and the bytes never cross the
//! Citadel server, so a hosted server meters them from these reports (its relay quota). Metering,
//! not diagnostics: a session reports whether or not it opted into telemetry, and only to a
//! server that says it meters TURN.
//!
//! Pure: each connection's counter is read through the closure it was tracked with, and the
//! shell decides when to take a report and where it goes.

use std::collections::{HashMap, HashSet};

/// Bytes sent and received, in that order.
pub type TurnBytes = (u64, u64);

/// Reads one connection's TURN counters, which only grow.
pub type TurnReader = Box<dyn Fn() -> TurnBytes + Send + Sync>;

struct Tracked {
    read: TurnReader,
    reported: TurnBytes,
}

#[derive(Default)]
pub struct TurnLedger {
    connections: HashMap<(u64, u64), Tracked>,
    /// Growth of connections replaced since the session's last report, not yet reported.
    carried: HashMap<u64, TurnBytes>,
    /// Sessions whose server meters TURN, from its consent answer.
    metered: HashSet<u64>,
}

fn growth(read: &TurnReader, reported: TurnBytes) -> (TurnBytes, TurnBytes) {
    let now = read();
    let grown = (
        now.0.saturating_sub(reported.0),
        now.1.saturating_sub(reported.1),
    );
    (grown, now)
}

fn add(a: TurnBytes, b: TurnBytes) -> TurnBytes {
    (a.0.saturating_add(b.0), a.1.saturating_add(b.1))
}

impl TurnLedger {
    /// Whether `cid`'s server meters TURN; a session whose server does not is never reported.
    pub fn set_metered(&mut self, cid: u64, meters_turn: bool) {
        if meters_turn {
            self.metered.insert(cid);
        } else {
            self.metered.remove(&cid);
        }
    }

    /// Counts `cid`'s connection to `peer_cid` from its first byte. A connection it replaces
    /// (a reconnect to the same peer) keeps its unreported growth for the next report.
    pub fn track(&mut self, cid: u64, peer_cid: u64, read: TurnReader) {
        let replaced = self.connections.insert(
            (cid, peer_cid),
            Tracked {
                read,
                reported: (0, 0),
            },
        );
        if let Some(old) = replaced {
            let (grown, _) = growth(&old.read, old.reported);
            let carried = self.carried.entry(cid).or_default();
            *carried = add(*carried, grown);
        }
    }

    /// The session ended: nothing more is counted or reported for it.
    pub fn forget(&mut self, cid: u64) {
        self.connections.retain(|(session, _), _| *session != cid);
        self.carried.remove(&cid);
        self.metered.remove(&cid);
    }

    /// Every metered session with growth since its last report, and that growth, which now
    /// counts as reported.
    pub fn take(&mut self) -> Vec<(u64, TurnBytes)> {
        let mut out: HashMap<u64, TurnBytes> = HashMap::new();
        for ((cid, _), tracked) in self.connections.iter_mut() {
            if !self.metered.contains(cid) {
                continue;
            }
            let (grown, now) = growth(&tracked.read, tracked.reported);
            tracked.reported = now;
            let total = out.entry(*cid).or_default();
            *total = add(*total, grown);
        }
        for cid in &self.metered {
            if let Some(carried) = self.carried.remove(cid) {
                let total = out.entry(*cid).or_default();
                *total = add(*total, carried);
            }
        }
        out.into_iter().filter(|(_, b)| *b != (0, 0)).collect()
    }
}

/// `bytes` as reports of at most `max` per direction each, so no single report is refused.
pub fn split_report(bytes: TurnBytes, max: u64) -> Vec<TurnBytes> {
    let mut left = bytes;
    let mut out = Vec::new();
    while left != (0, 0) {
        let part = (left.0.min(max), left.1.min(max));
        out.push(part);
        left = (left.0 - part.0, left.1 - part.1);
    }
    out
}

#[cfg(test)]
#[path = "turn_ledger_tests.rs"]
mod tests;

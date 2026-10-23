//! The uplink's TURN usage reports: each metered session's TURN growth goes to its own server on
//! every flush, whether or not the session opted into telemetry (see `turn_ledger`).

use super::{SessionLink, TelemetryUplink, LOG_TARGET};
use crate::telemetry::turn_ledger::TurnReader;
use citadel_sdk::logging::warn;

impl TelemetryUplink {
    /// Counts `cid`'s connection to `peer_cid` through `read`, its TURN counters.
    pub fn track_turn(&self, cid: u64, peer_cid: u64, read: TurnReader) {
        self.turn.lock().track(cid, peer_cid, read);
    }

    /// The session ended: its TURN bytes are no longer counted.
    pub fn forget_turn(&self, cid: u64) {
        self.turn.lock().forget(cid);
    }

    /// Sends each metered session's growth since the last flush. A report that cannot be framed
    /// or sent is dropped, not retried: the bytes it carried go unmetered. Accepted for v1, as
    /// it under-meters, in the customer's favour; the ledger has already counted them reported.
    pub(super) async fn flush_turn(&self, link: &dyn SessionLink) {
        let due = self.turn.lock().take();
        for (cid, bytes) in due {
            let frames = match self.codec.turn_usage_frames(bytes) {
                Ok(frames) => frames,
                Err(err) => {
                    warn!(target: LOG_TARGET, "TURN usage for session {cid} dropped: {err}");
                    continue;
                }
            };
            for frame in frames {
                if let Err(err) = link.send_to_server(cid, frame).await {
                    warn!(target: LOG_TARGET, "TURN usage report for session {cid} dropped: {err}");
                    break;
                }
            }
        }
    }
}

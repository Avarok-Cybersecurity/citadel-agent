//! Reports a peer connection's path to the application as it changes.
//!
//! A peer connection is delivered as soon as it works over the server relay; the SDK upgrades it
//! to a direct (or TURN) path in the background, and falls back to the relay if that path is
//! lost. `PeerConnectSuccess` carries the path at delivery; every later change is forwarded as a
//! `PeerPathChangedNotification`, routed to whichever localhost connection owns the session at
//! that moment.

use crate::kernel::requests::peer::turn::path_report;
use crate::kernel::session_route::SessionRoute;
use citadel_internal_service_types::{
    InternalServiceResponse, P2pPathReport, PeerPathChangedNotification,
};
use citadel_sdk::logging::info;
use citadel_sdk::prelude::{P2pPathCell, P2pPathStatus};
use tokio::sync::watch;

/// A subscription taken before the connection is reported, so no change can fall between the
/// report and the first forwarded notification.
pub(crate) struct PathWatch {
    rx: watch::Receiver<P2pPathStatus>,
}

impl PathWatch {
    pub(crate) fn new(cell: &P2pPathCell) -> Self {
        Self {
            rx: cell.subscribe(),
        }
    }

    /// The path and upgrade state to report with `PeerConnectSuccess`.
    pub(crate) fn current(&self) -> (P2pPathReport, bool) {
        let status = *self.rx.borrow();
        (path_report(status.path), status.upgrading)
    }

    /// Forwards every later change until the connection closes.
    pub(crate) fn forward(mut self, cid: u64, peer_cid: u64, route: SessionRoute) {
        tokio::spawn(async move {
            while self.rx.changed().await.is_ok() {
                let status = *self.rx.borrow_and_update();
                let Some(notification) = notification(cid, peer_cid, status) else {
                    break;
                };
                if route.send(notification).is_empty() {
                    info!(target: "citadel", "[PeerPath] No localhost connection owns CID {cid}; path change for peer {peer_cid} dropped");
                }
            }
        });
    }
}

/// The notification for `status`, or `None` once the connection has closed (its end is reported
/// by the disconnect path, not as a path).
pub(crate) fn notification(
    cid: u64,
    peer_cid: u64,
    status: P2pPathStatus,
) -> Option<InternalServiceResponse> {
    if status.closed {
        return None;
    }
    Some(InternalServiceResponse::PeerPathChangedNotification(
        PeerPathChangedNotification {
            cid,
            peer_cid,
            path: path_report(status.path),
            upgrading: status.upgrading,
            request_id: None,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use citadel_sdk::prelude::P2pPath;

    fn status(path: P2pPath, upgrading: bool, closed: bool) -> P2pPathStatus {
        P2pPathStatus {
            path,
            relayed_both: false,
            upgrading,
            closed,
        }
    }

    #[test]
    fn a_change_is_reported_with_its_session_and_peer() {
        let Some(InternalServiceResponse::PeerPathChangedNotification(n)) =
            notification(7, 9, status(P2pPath::Direct, false, false))
        else {
            panic!("expected a PeerPathChangedNotification")
        };
        assert_eq!((n.cid, n.peer_cid), (7, 9));
        assert_eq!(n.path, P2pPathReport::Direct);
        assert!(!n.upgrading);
        assert_eq!(n.request_id, None);
    }

    #[test]
    fn a_closed_connection_reports_nothing() {
        assert!(notification(7, 9, status(P2pPath::ServerRelay, false, true)).is_none());
    }
}

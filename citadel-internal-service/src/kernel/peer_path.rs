//! Reports a peer connection's path to the application as it changes.
//!
//! A peer connection is delivered as soon as it works over the server relay; the SDK upgrades it
//! to a direct (or TURN) path in the background, and falls back to the relay if that path is
//! lost. `PeerConnectSuccess` carries the path at delivery; every later change is forwarded as a
//! `PeerPathChangedNotification`, routed to whichever localhost connection owns the session at
//! that moment.

use crate::kernel::requests::peer::turn::path_report;
use crate::kernel::session_route::SessionRoute;
use crate::kernel::supervisor::{Signal, Supervisors};
use crate::kernel::Connection;
use citadel_internal_service_types::{
    InternalServiceResponse, P2pPathReport, PeerPathChangedNotification,
};
use citadel_sdk::logging::info;
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::{P2pPathCell, P2pPathStatus, Ratchet, UdpChannel};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedReceiver;
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
    ///
    /// The account's supervisor hears the connection come up, and each change of its path
    /// (kernel/supervisor); its loss comes from the disconnect path, which knows whether
    /// the user ended it.
    pub(crate) fn forward(
        mut self,
        cid: u64,
        peer_cid: u64,
        route: SessionRoute,
        supervisors: Arc<Supervisors>,
    ) {
        supervisors.signal(
            cid,
            Signal::PeerUp(peer_cid, path_report(self.rx.borrow().path)),
        );
        tokio::spawn(async move {
            while self.rx.changed().await.is_ok() {
                let status = *self.rx.borrow_and_update();
                let Some(notification) = notification(cid, peer_cid, status) else {
                    break;
                };
                supervisors.signal(cid, Signal::PeerPath(peer_cid, path_report(status.path)));
                if route.send(notification).is_empty() {
                    info!(target: "citadel", "[PeerPath] No localhost connection owns CID {cid}; path change for peer {peer_cid} dropped");
                }
            }
        });
    }
}

/// A recovery that began with UDP restores the UDP channel (`PathControl::upgrade`); each one
/// replaces the channel that ended with the route it rode, and is offered to the peer's next
/// call. A peer in a call keeps its own transport: the media pump does not yet move to a
/// restored one.
pub(crate) fn adopt_restored_udp<R: Ratchet>(
    map: Arc<RwLock<HashMap<u64, Connection<R>>>>,
    cid: u64,
    peer_cid: u64,
    mut restored: UnboundedReceiver<UdpChannel<R>>,
) {
    tokio::spawn(async move {
        while let Some(channel) = restored.recv().await {
            let adopted = map
                .write()
                .get_mut(&cid)
                .is_some_and(|conn| conn.offer_restored_udp(peer_cid, channel));
            if !adopted {
                warn!(target: "citadel", "[PeerPath] {cid}: a restored UDP channel for {peer_cid} was not taken (a call is live, or the peer is gone)");
            }
        }
    });
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

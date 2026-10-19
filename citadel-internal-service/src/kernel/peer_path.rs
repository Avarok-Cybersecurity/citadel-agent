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
        crate::telemetry::span::spawn_in_current_span(async move {
            let mut previous = *self.rx.borrow();
            while self.rx.changed().await.is_ok() {
                let status = *self.rx.borrow_and_update();
                let Some(notification) = notification(cid, peer_cid, status) else {
                    break;
                };
                if lost_its_direct_route(previous, status) {
                    ask_for_udp_back(&supervisors, cid, peer_cid);
                }
                previous = status;
                supervisors.signal(cid, Signal::PeerPath(peer_cid, path_report(status.path)));
                if route.send(notification).is_empty() {
                    info!(target: "citadel", "[PeerPath] No localhost connection owns CID {cid}; path change for peer {peer_cid} dropped");
                }
            }
        });
    }
}

/// Whether this change is a live connection's direct (or TURN) route ending: it fell back to
/// the server relay, and the UDP channel went with the route.
pub(crate) fn lost_its_direct_route(previous: P2pPathStatus, now: P2pPathStatus) -> bool {
    previous.is_p2p() && !now.is_p2p() && !now.closed
}

/// The SDK retries a lost route on its own, a couple of seconds later, but a retry brings the
/// UDP channel back only when asked; otherwise the route returns without it and the call on it
/// stays silent for as long as the connection lasts. Asking now, before that first retry's
/// rendezvous, makes it the one that restores UDP (a connection that began without UDP is not
/// given one: the SDK restores only what the connection began with).
fn ask_for_udp_back(supervisors: &Supervisors, cid: u64, peer_cid: u64) {
    let Some(control) = supervisors.path_control(cid, peer_cid) else {
        return;
    };
    match control.upgrade(true) {
        Ok(()) => {
            info!(target: "citadel", "[PeerPath] {cid}: route to {peer_cid} fell back; asked for its UDP channel back")
        }
        Err(err) => {
            warn!(target: "citadel", "[PeerPath] {cid}: route to {peer_cid} fell back; UDP cannot be restored: {err:?}")
        }
    }
}

/// A recovery that began with UDP restores the UDP channel (`PathControl::upgrade`); each one
/// replaces the channel that ended with the route it rode. A live call moves to it, and
/// otherwise it is offered to the peer's next call.
pub(crate) fn adopt_restored_udp<R: Ratchet>(
    map: Arc<RwLock<HashMap<u64, Connection<R>>>>,
    cid: u64,
    peer_cid: u64,
    mut restored: UnboundedReceiver<UdpChannel<R>>,
) {
    crate::telemetry::span::spawn_in_span(
        crate::telemetry::span::peer_span(cid, peer_cid),
        async move {
            while let Some(channel) = restored.recv().await {
                let adopted = map
                    .write()
                    .get_mut(&cid)
                    .is_some_and(|conn| conn.offer_restored_udp(peer_cid, channel));
                if !adopted {
                    warn!(target: "citadel", "[PeerPath] {cid}: a restored UDP channel for {peer_cid} was not taken (an open is mid-await, or the peer is gone)");
                }
            }
        },
    );
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
    fn only_a_fall_back_from_a_p2p_route_asks_for_udp() {
        let relay = status(P2pPath::ServerRelay, true, false);
        let direct = status(P2pPath::Direct, false, false);
        assert!(lost_its_direct_route(direct, relay), "a fall-back");
        assert!(!lost_its_direct_route(relay, direct), "the first upgrade");
        assert!(!lost_its_direct_route(relay, relay), "still on the relay");
        let closed = status(P2pPath::ServerRelay, false, true);
        assert!(
            !lost_its_direct_route(direct, closed),
            "the connection ended"
        );
    }

    #[test]
    fn a_closed_connection_reports_nothing() {
        assert!(notification(7, 9, status(P2pPath::ServerRelay, false, true)).is_none());
    }
}

//! A peer redialled after its connection dropped, then a late report of the OLD connection's
//! end. The report goes through the decision `cleanup_reported_peer` makes
//! (`reconnect::instance::take_reported_peer`), and a removal reaches the core as
//! `responses/peer_event.rs` sends it, as `PeerLost`.

use super::super::types::{Command, DialOutcome, Input};
use super::tests::{core, secs, tick, PEER};
use super::tests_peers::{dials, up, QUIET};
use super::Core;
use crate::kernel::reconnect::instance::{take_reported_peer, Instance, PeerEnd, PeerReport};
use citadel_internal_service_types::P2pPathReport;
use citadel_sdk::prelude::Ticket;
use std::collections::HashMap;

const OLD: Instance = Ticket(41);
const NEW: Instance = Ticket(42);

/// The session's peers, each with the connection its sink is.
type Peers = HashMap<u64, Option<Instance>>;

fn reported(peers: &mut Peers, core: &mut Core, at: u64, connection: Option<Instance>) -> bool {
    match take_reported_peer(peers, PEER, PeerEnd::Reported(connection), |peer| *peer) {
        PeerReport::Removed(_) => {
            core.step(Input::PeerLost {
                now: secs(at),
                peer: PEER,
            });
            true
        }
        PeerReport::Kept { .. } | PeerReport::Absent => false,
    }
}

#[test]
fn a_late_report_of_the_replaced_connection_keeps_the_redialled_peer() {
    let mut core = core(QUIET);
    let mut peers = Peers::from([(PEER, Some(OLD))]);
    up(&mut core, 1, P2pPathReport::ServerRelay);

    // The connection drops and is reported: the peer goes and is redialled.
    assert!(reported(&mut peers, &mut core, 10, Some(OLD)));
    assert!(peers.is_empty());
    assert_eq!(dials(&tick(&mut core, 11)), 1);
    core.step(Input::DialResult {
        now: secs(12),
        peer: PEER,
        outcome: DialOutcome::Connected,
    });
    peers.insert(PEER, Some(NEW));
    up(&mut core, 12, P2pPathReport::ServerRelay);

    // The old connection's end is reported again, late (another path of the SDK's).
    assert!(
        !reported(&mut peers, &mut core, 20, Some(OLD)),
        "the redialled peer was removed for the old connection's end"
    );
    // And a report naming no connection (the server's notice that the peer's old session
    // ended; CI run 37295484651 delivered one after the named ones) leaves it too.
    assert!(
        !reported(&mut peers, &mut core, 21, None),
        "the redialled peer was removed by a report that names no connection"
    );
    assert_eq!(peers.get(&PEER), Some(&Some(NEW)));
    let quiet = tick(&mut core, 60);
    assert_eq!(
        dials(&quiet),
        0,
        "nothing lost, nothing to redial: {quiet:?}"
    );

    // The new connection's own end still removes it, and it is redialled.
    assert!(reported(&mut peers, &mut core, 70, Some(NEW)));
    assert!(peers.is_empty());
    // Its backoff is not yet forgiven (up for under `stable_after`): the second wait is 2 s.
    let redial: Vec<Command> = tick(&mut core, 72);
    assert_eq!(dials(&redial), 1, "{redial:?}");
}

#[test]
fn who_ends_which_connection() {
    let take = |current: Option<Instance>, end| {
        let mut peers = Peers::from([(PEER, current)]);
        take_reported_peer(&mut peers, PEER, end, |p| *p)
    };
    assert_eq!(
        take(Some(NEW), PeerEnd::Requested),
        PeerReport::Removed(Some(NEW)),
        "the user ends whichever connection the peer has"
    );
    assert_eq!(
        take(Some(NEW), PeerEnd::Reported(Some(NEW))),
        PeerReport::Removed(Some(NEW))
    );
    assert_eq!(
        take(Some(NEW), PeerEnd::Reported(Some(OLD))),
        PeerReport::Kept { current: Some(NEW) }
    );
    assert_eq!(
        take(Some(NEW), PeerEnd::Reported(None)),
        PeerReport::Kept { current: Some(NEW) },
        "a report naming no connection does not end a named one"
    );
    assert_eq!(
        take(None, PeerEnd::Reported(None)),
        PeerReport::Removed(None),
        "a connection the SDK named nothing for is ended by any report"
    );
    assert_eq!(
        take(None, PeerEnd::Reported(Some(OLD))),
        PeerReport::Removed(None)
    );
    let mut none = Peers::new();
    assert_eq!(
        take_reported_peer(&mut none, PEER, PeerEnd::Requested, |p| *p),
        PeerReport::Absent
    );
}

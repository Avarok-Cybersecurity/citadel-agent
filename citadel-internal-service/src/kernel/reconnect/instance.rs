//! Which SDK session a drop report is about.
//!
//! A CID is permanent, so a session the agent replaced (the supervisor abandoned it, or a
//! reconnect installed a new one) has the same CID as its replacement, and its teardown can
//! be reported after the replacement is up. Measured on CI: an SDK hole-punch task begun
//! before the abandon held the old session for its 30 s timeout, then dropped it, and the
//! `Disconnect` that drop sent named the CID of the healthy new link. Taken by CID alone for
//! that link dropping, it reported Healing again and started a reconnect the SDK refused for
//! as long as it held the live session ("Session ... is already connected"), so the peers
//! never came back.
//!
//! The SDK stamps a session-end report with the instance it is about
//! (`Disconnect::disconnect_token`: the session's kernel ticket, which is also its C2S
//! channel's id). The entry records the instance its link is, and a report of another
//! instance is not about this link.

use super::policy::{self, DropAction};
use super::LinkState;
use citadel_sdk::prelude::Ticket;
use std::collections::HashMap;

/// The SDK session instance a link is: its C2S channel's id.
pub(crate) type Instance = Ticket;

/// What a drop report means for the entry whose link is `current`: `None` when it reports
/// another instance's end. A report naming no instance (the SDK names none in its answers
/// to an abandon or a disconnect request) is taken as the entry's, as it always was.
pub(crate) fn on_reported_drop(
    link: LinkState,
    current: Instance,
    reported: Option<Instance>,
) -> Option<DropAction> {
    names_this_instance(Some(current), reported).then(|| policy::on_unrequested_drop(link))
}

/// Whether a report that names `reported` is about the connection that is `current`. Either
/// unknown (a report naming none, a channel from an SDK that names none) cannot tell them
/// apart, so it is taken as about it, as every report was before.
pub(crate) fn names_this_instance(current: Option<Instance>, reported: Option<Instance>) -> bool {
    match (current, reported) {
        (Some(current), Some(reported)) => current == reported,
        _ => true,
    }
}

/// What a peer's disconnect report did to the session's peers.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PeerReport<P> {
    /// It named the connection the peer has: the peer is gone.
    Removed(P),
    /// It named a connection the peer replaced (a redial); the peer is kept.
    Replaced { current: Option<Instance> },
    /// The peer was not there.
    Absent,
}

/// Remove `peer` for a disconnect report that names `reported`, unless the peer's connection
/// (`instance_of`) is another.
pub(crate) fn take_reported_peer<P>(
    peers: &mut HashMap<u64, P>,
    peer: u64,
    reported: Option<Instance>,
    instance_of: impl FnOnce(&P) -> Option<Instance>,
) -> PeerReport<P> {
    let Some(current) = peers.get(&peer).map(instance_of) else {
        return PeerReport::Absent;
    };
    if !names_this_instance(current, reported) {
        return PeerReport::Replaced { current };
    }
    peers
        .remove(&peer)
        .map_or(PeerReport::Absent, PeerReport::Removed)
}

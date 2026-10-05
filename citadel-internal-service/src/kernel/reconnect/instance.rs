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

/// Why a peer's connection is to be ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeerEnd {
    /// The user ended it: whichever connection the peer has.
    Requested,
    /// The SDK reported an end naming this connection, or none
    /// (Citadel-Protocol `PeerSignal::Disconnect::disconnect_token`). A named report ends only
    /// the connection it names. An unnamed one (the server's notice that the peer's whole
    /// session ended, or a peer whose SDK names none) ends only a connection that is itself
    /// unnamed: a named connection's own end is always reported, named, so an unnamed report
    /// arriving after a redial must not take the new connection with it.
    Reported(Option<Instance>),
}

/// Whether `end` ends the connection that is `current`.
pub(crate) fn ends_this_connection(current: Option<Instance>, end: PeerEnd) -> bool {
    match (end, current) {
        (PeerEnd::Requested, _) | (PeerEnd::Reported(_), None) => true,
        (PeerEnd::Reported(Some(reported)), Some(current)) => reported == current,
        (PeerEnd::Reported(None), Some(_)) => false,
    }
}

/// What ending a peer's connection did to the session's peers.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PeerReport<P> {
    /// It ended the connection the peer has: the peer is gone.
    Removed(P),
    /// It is not about the connection the peer has now (a redial replaced the one it names, or
    /// it names none); the peer is kept.
    Kept { current: Option<Instance> },
    /// The peer was not there.
    Absent,
}

/// Remove `peer` for `end`, unless it is not about the peer's connection (`instance_of`).
pub(crate) fn take_reported_peer<P>(
    peers: &mut HashMap<u64, P>,
    peer: u64,
    end: PeerEnd,
    instance_of: impl FnOnce(&P) -> Option<Instance>,
) -> PeerReport<P> {
    let Some(current) = peers.get(&peer).map(instance_of) else {
        return PeerReport::Absent;
    };
    if !ends_this_connection(current, end) {
        return PeerReport::Kept { current };
    }
    peers
        .remove(&peer)
        .map_or(PeerReport::Absent, PeerReport::Removed)
}

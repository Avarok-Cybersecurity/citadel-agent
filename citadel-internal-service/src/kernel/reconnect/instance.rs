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
    reported
        .is_none_or(|reported| reported == current)
        .then(|| policy::on_unrequested_drop(link))
}

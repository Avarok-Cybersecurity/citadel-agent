//! Views derived per viewer, never stored (`vfs-model.md` §4, §8; `vfs-ui.md` §3, §7;
//! `vfs-outsiders.md` §5).

mod access;
mod places;

pub use access::{effective_access, names_seen_by, Access, NamesSeenBy};
pub use places::{outsider_view, recycle_bin, shared_with_me, BinItem, SharedItem};

use crate::model::Pointee;

/// The tile badge for a viewer (`vfs-ui.md` §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Badge {
    /// Green shield: "Encrypted. Only you can open it."
    Owner,
    /// Golden lock: "You store this for <owner>. You cannot open it."
    Host,
    /// Grey lock: "Encrypted for <owner>."
    Other,
}

/// `None` for anything but a `Revfs` pointee.
pub fn badge(pointee: &Pointee, me: &crate::principal::Principal) -> Option<Badge> {
    let Pointee::Revfs { owner, host, .. } = pointee else {
        return None;
    };
    Some(if me.cid() == Some(*owner) {
        Badge::Owner
    } else if me == host {
        Badge::Host
    } else {
        Badge::Other
    })
}

//! `authorize`, the one permission function (`vfs-authorize.md`): every op by every actor class
//! in every volume kind, and the three reads. The first failing check is the answer, in the
//! order of §2, so every caller names the same refusal.

mod actor;
mod member;
mod outsider;
mod read;
mod rules;

pub(crate) use outsider::admission_charged;
pub use read::{may_list, may_read_bytes, may_see};

use crate::inode::InodeId;
use crate::model::Bit;
use crate::op::Op;
use crate::principal::Principal;
use crate::reduce::Tree;
use crate::volume::VolumeKind;

/// What one member holds in a volume (`vfs-authorize.md` §1, by kind).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoleMask {
    pub view: bool,
    pub upload: bool,
    pub download: bool,
    pub manage_files: bool,
    pub share_links: bool,
}

impl RoleMask {
    /// Every bit: what a member holds in a Pair or Device volume, and a Group member who is
    /// not the group's owner holds all but `manage_files`.
    pub const fn peer(manage_files: bool) -> Self {
        Self {
            view: true,
            upload: true,
            download: true,
            manage_files,
            share_links: true,
        }
    }
}

/// The volume as of the op's position, from the caller (the crate knows no kernel).
pub struct VolumeCtx<'a> {
    pub kind: VolumeKind,
    /// At the op's position (`vfs-model.md` §1).
    pub is_member: &'a dyn Fn(u64) -> bool,
    pub role: &'a dyn Fn(u64) -> RoleMask,
    pub retention_ms: u64,
    /// The kernel's clock; elsewhere the op's own `at_ms`.
    pub now_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum RoleNeed {
    View,
    Upload,
    Download,
    ShareLinks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Cap {
    Bytes,
    Entries,
    Rate,
}

/// Every refusal `authorize` and the reads give.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Denied {
    ServerOpKind,
    NoServerHere,
    /// `at` is the entry that lacks the bit.
    Mode {
        need: Bit,
        at: InodeId,
    },
    Role {
        need: RoleNeed,
    },
    NotOwner,
    OwnerNotAuthor,
    NewOwnerNotMember,
    NotAFile,
    NotRemoved,
    RevfsOwnerOnly,
    RevfsHost,
    HostCannotPurge,
    NotAdmitted,
    OutsiderNot,
    OwnerNotAdmitter,
    AdmissionCap(Cap),
    AlreadyMember,
    NoOutsidersHere,
    ClaimsExhausted,
    BadClaim,
}

/// Whether the op's author may make this op against `tree` as of its position.
///
/// The entries the op is judged on must exist; the reducer checks that first (R5, R9) and a
/// missing one is refused here without granting anything. A redacted op is never authorized.
pub fn authorize(tree: &Tree, ctx: &VolumeCtx, op: &Op) -> Result<(), Denied> {
    let Some(body) = &op.body else {
        return Err(Denied::NotAdmitted);
    };
    match &op.header.author {
        Principal::Server => member::server(tree, ctx, op, body),
        Principal::Member { cid } if (ctx.is_member)(*cid) => {
            member::member(tree, ctx, *cid, op, body)
        }
        outsider => outsider::outsider(tree, ctx, outsider, op, body),
    }
}

/// The class a principal acts in, for the views that state it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Server,
    Administrator,
    Owner,
    Member,
    Outsider,
}

/// The class of `actor` judged on `entry` (`vfs-authorize.md` §2).
pub fn class_of(tree: &Tree, ctx: &VolumeCtx, actor: &Principal, entry: &InodeId) -> Class {
    match actor {
        Principal::Server => Class::Server,
        Principal::Member { cid } if (ctx.is_member)(*cid) => {
            actor::member_class(tree, ctx, *cid, entry)
        }
        _ => Class::Outsider,
    }
}

/// Whom an outsider's new entry under `at` must be owned by (its admitter), if it is admitted
/// there; for anyone else, `None`.
pub fn admitter_at(tree: &Tree, actor: &Principal, at: &InodeId, now_ms: u64) -> Option<Principal> {
    actor::admission_roots(tree, actor, at, now_ms)
        .into_iter()
        .next()
        .map(|(_, admission)| admission.by.clone())
}

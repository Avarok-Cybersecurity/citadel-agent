//! The reducer (`vfs-ops.md` §4): one fold for every volume kind, each op at its position.

mod apply;
mod effects;
mod group;
mod group_signed;
mod merge;
mod ordered;
mod restore;
mod rules;
mod shown;
mod tree;

pub use apply::Applied;
pub use group::{GroupEvent, GroupEventKind, GroupFold};
pub use group_signed::{GroupVerifier, KeyAnswer, Verdict};
pub use merge::MergeFold;
pub use ordered::OrderedFold;
pub use restore::RestoreError;
pub use rules::LimitHit;
pub use shown::numbered;
pub use tree::{Admission, Entry, EntryState, Place, Tree, Usage};

use crate::authorize::{Denied, VolumeCtx};
use crate::op::Stamp;

/// Where an op is folded (`vfs-ops.md` R1). One volume uses one form.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Position {
    /// The kernel's `seq` (Personal, Node), from 1.
    Seq(u64),
    /// The group's broadcast `seq` and the op's index in it.
    Group { seq: u64, index: u32 },
    /// Merge volumes: the stamp.
    Stamp(Stamp),
}

/// The op that last won a register.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Won {
    pub position: Position,
    pub stamp: Stamp,
    pub at_ms: u64,
}

/// A last-writer-wins register and the write that won it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reg<T> {
    pub value: T,
    pub won: Won,
}

/// Why an op was not applied (`vfs-ops.md` §4: exactly these).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SkipReason {
    NoParent,
    Cycle,
    Limit,
    NotEmpty,
    Purged,
    Denied(Denied),
    Duplicate,
    /// Group volumes (`vfs-group.md` §2) also:
    Invalid,
    NotAuthor,
    KeyChanged,
    BadSignature,
    StalePrev,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub op_id: [u8; 16],
    pub op_hash: [u8; 32],
    pub reason: SkipReason,
}

/// Who judges an op with `authorize` when it is folded (`vfs-ops.md` R10).
pub enum Judge<'c, 'a> {
    /// Pair, Device, Group: every op.
    Every(&'c VolumeCtx<'a>),
    /// Personal, Node replicas: only ops authored by `Server` (the kernel judged the rest).
    ServerOnly(&'c VolumeCtx<'a>),
}

impl<'a> Judge<'_, 'a> {
    pub(crate) fn ctx(&self) -> &VolumeCtx<'a> {
        match self {
            Self::Every(ctx) | Self::ServerOnly(ctx) => ctx,
        }
    }
}

/// What a replica's fold can say about a volume as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FoldHalt {
    /// A header with `v != 1` or an unknown kind (`vfs-ops.md` R12): fold no further.
    UpdateNeeded,
    /// Positions must rise.
    OutOfOrder,
}

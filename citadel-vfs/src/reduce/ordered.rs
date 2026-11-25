//! Personal and Node volumes (`vfs-ops.md` R1, R10): the kernel's sequence, from 1 without
//! gaps. Members' ops were judged by the kernel; a `Server` op is judged here too.

use super::apply::Applied;
use super::{FoldHalt, Judge, Position, SkipReason, Tree};
use crate::authorize::VolumeCtx;
use crate::op::Op;
use crate::volume::VolumeId;

#[derive(Debug, Clone)]
pub struct OrderedFold {
    tree: Tree,
    last_seq: u64,
}

impl OrderedFold {
    /// `None` unless the volume is a Personal or Node volume.
    pub fn new(volume: VolumeId) -> Option<Self> {
        volume.has_kernel().then(|| Self {
            tree: Tree::for_volume(volume),
            last_seq: 0,
        })
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    /// Fold the op at `seq`, which must be the next one. `ctx` is the volume as of `seq` (the
    /// kernel's member list, the op's `at_ms` as the clock on a replica).
    pub fn apply(
        &mut self,
        seq: u64,
        op: &Op,
        ctx: &VolumeCtx,
    ) -> Result<Result<Applied, SkipReason>, FoldHalt> {
        if seq != self.last_seq.saturating_add(1) {
            return Err(FoldHalt::OutOfOrder);
        }
        self.last_seq = seq;
        Ok(self
            .tree
            .apply(Position::Seq(seq), op, &Judge::ServerOnly(ctx)))
    }

    /// The kernel's own fold: it judged every op at admission, so nothing is re-judged here
    /// beyond what `apply` does for `Server` ops.
    pub fn into_tree(self) -> Tree {
        self.tree
    }
}

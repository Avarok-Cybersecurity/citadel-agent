//! Merge volumes (Pair, Device; `vfs-ops.md` R1, R2, R10; `vfs-outsiders.md` §2): the op set
//! folds in stamp order, so any arrival order gives one tree. Ops arrive one at a time: one
//! later than everything folded applies at once, any other refolds the set.

use super::apply::Applied;
use super::{Judge, Position, SkipReason, Tree};
use crate::authorize::{Denied, RoleMask, VolumeCtx};
use crate::limits::PEER_RETENTION_MS;
use crate::op::{Body, Op, Stamp};
use crate::principal::Principal;
use crate::volume::{VolumeId, VolumeMode};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct MergeFold {
    volume: VolumeId,
    /// The kept op of each `op_id`, by stamp.
    ops: BTreeMap<Stamp, Op>,
    by_id: BTreeMap<[u8; 16], Stamp>,
    /// R2: ops dropped for a smaller `op_hash` under their `op_id`.
    dropped: Vec<Op>,
    /// Per dismissed outsider principal, the `op_hash`es its dismisser accepted.
    cuts: BTreeMap<Principal, BTreeSet<[u8; 32]>>,
    tree: Tree,
}

/// The members of a Merge volume.
fn members(volume: &VolumeId) -> Vec<u64> {
    match volume {
        VolumeId::Pair { lo, hi } => vec![*lo, *hi],
        VolumeId::Device { cid } => vec![*cid],
        _ => Vec::new(),
    }
}

impl MergeFold {
    /// `None` unless the volume is a Merge volume.
    pub fn new(volume: VolumeId) -> Option<Self> {
        (volume.mode() == VolumeMode::Merge).then(|| Self {
            tree: Tree::for_volume(volume.clone()),
            volume,
            ops: BTreeMap::new(),
            by_id: BTreeMap::new(),
            dropped: Vec::new(),
            cuts: BTreeMap::new(),
        })
    }

    /// One fold of a whole set, in any order.
    pub fn fold(volume: VolumeId, ops: impl IntoIterator<Item = Op>) -> Option<Tree> {
        let mut fold = Self::new(volume)?;
        for op in ops {
            fold.hold(op);
        }
        fold.refold();
        Some(fold.tree)
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// Fold one more op.
    pub fn insert(&mut self, op: Op) {
        let stamp = op.header.stamp();
        let later = self.ops.keys().next_back().is_none_or(|last| stamp > *last);
        let simple = later && !matches!(op.body, Some(Body::Dismiss { .. }));
        if !self.hold(op) {
            return self.refold();
        }
        match self.ops.get(&stamp).cloned() {
            Some(op) if simple => self.fold_one(&op),
            _ => self.refold(),
        }
    }

    /// Keep `op` in the set by R2; `false` if the set changed in a way that needs a refold.
    fn hold(&mut self, op: Op) -> bool {
        let stamp = op.header.stamp();
        match self.by_id.get(&op.header.op_id).cloned() {
            None => {
                self.by_id.insert(op.header.op_id, stamp.clone());
                self.ops.insert(stamp, op);
                true
            }
            Some(kept) => {
                let held = &self.ops[&kept];
                if held.op_hash == op.op_hash {
                    return true;
                }
                if op.op_hash < held.op_hash {
                    let old = self.ops.remove(&kept);
                    self.dropped.extend(old);
                    self.by_id.insert(op.header.op_id, stamp.clone());
                    self.ops.insert(stamp, op);
                } else {
                    self.dropped.push(op);
                }
                false
            }
        }
    }

    fn fold_one(&mut self, op: &Op) {
        let cut = self
            .cuts
            .get(&op.header.author)
            .is_some_and(|accepted| !accepted.contains(&op.op_hash));
        if cut {
            self.tree
                .record_skip(op, SkipReason::Denied(Denied::NotAdmitted));
            return;
        }
        let ids = members(&self.volume);
        let is_member = |cid: u64| ids.contains(&cid);
        let role = |_: u64| RoleMask::peer(false);
        let ctx = VolumeCtx {
            kind: self.volume.kind(),
            is_member: &is_member,
            role: &role,
            retention_ms: PEER_RETENTION_MS,
            now_ms: op.header.at_ms,
        };
        let position = Position::Stamp(op.header.stamp());
        let _: Result<Applied, SkipReason> = self.tree.apply(position, op, &Judge::Every(&ctx));
    }

    /// Fold the whole set again, until the dismissals' cuts (`Dismiss.lasts`) stop growing.
    fn refold(&mut self) {
        self.cuts.clear();
        loop {
            self.tree = Tree::for_volume(self.volume.clone());
            // R2's dropped ops first, so that later ops append in stamp order however they came.
            self.dropped.sort_by_key(|o| (o.header.op_id, o.op_hash));
            for op in &self.dropped {
                self.tree.record_dropped(op);
            }
            let ops: Vec<Op> = self.ops.values().cloned().collect();
            for op in &ops {
                self.fold_one(op);
            }
            let cuts = self.cuts_of_applied();
            if cuts == self.cuts {
                return;
            }
            // Only ever narrower than before, so this ends.
            self.cuts = cuts;
        }
    }

    /// For every applied `Dismiss`, each listed author's accepted ops: the named one and its
    /// `prev` ancestors among the held ops.
    fn cuts_of_applied(&self) -> BTreeMap<Principal, BTreeSet<[u8; 32]>> {
        let skipped: BTreeSet<[u8; 32]> = self.tree.skipped().iter().map(|s| s.op_hash).collect();
        let by_hash: BTreeMap<[u8; 32], &Op> = self.ops.values().map(|o| (o.op_hash, o)).collect();
        let mut cuts: BTreeMap<Principal, BTreeSet<[u8; 32]>> = self.cuts.clone();
        for op in self.ops.values().filter(|o| !skipped.contains(&o.op_hash)) {
            let Some(Body::Dismiss { lasts, .. }) = &op.body else {
                continue;
            };
            for (author, last) in lasts {
                let mut accepted = BTreeSet::new();
                let mut at = *last;
                while let Some(hash) = at {
                    if !accepted.insert(hash) {
                        break;
                    }
                    at = by_hash.get(&hash).and_then(|o| o.header.prev);
                }
                let entry = cuts
                    .entry(author.clone())
                    .or_insert_with(|| accepted.clone());
                entry.retain(|h| accepted.contains(h));
            }
        }
        cuts
    }
}

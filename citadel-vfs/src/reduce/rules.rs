//! The reducer's own rules (`vfs-ops.md` R4–R9), apart from authorization.

use super::{SkipReason, Tree};
use crate::inode::InodeId;
use crate::limits::VolumeLimits;
use crate::op::{Body, Op};

/// Which limit an op would pass (`[vfs]` keys of the same names).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitHit {
    Entries,
    EntriesPerDir,
    Depth,
}

impl Tree {
    /// R4, R5 and R9's existence half: what must exist for the op to be judged at all.
    pub(super) fn existence(&self, op: &Op, body: &Body) -> Result<(), SkipReason> {
        let target = &op.header.target;
        if self.purged.contains(target) {
            return Err(SkipReason::Purged);
        }
        let parent_ok = |p: &InodeId| !self.purged.contains(p) && self.is_dir(p);
        match body {
            Body::Create { kind, parent, .. } => {
                if !parent_ok(parent) {
                    return Err(SkipReason::NoParent);
                }
                match self.entries.get(target) {
                    Some(existing) if existing.kind != *kind => Err(SkipReason::Duplicate),
                    _ => Ok(()),
                }
            }
            // R9: a Purge applies only if its target exists (and is empty, below).
            Body::Purge if !self.entries.contains_key(target) => Err(SkipReason::NotEmpty),
            _ if !self.entries.contains_key(target) => Err(SkipReason::NoParent),
            Body::Place { parent, .. } if !parent_ok(parent) => Err(SkipReason::NoParent),
            _ => Ok(()),
        }
    }

    /// R6 (no cycles), R7 (limits, where given) and R9 (purge only an empty entry).
    pub(super) fn shape(
        &self,
        op: &Op,
        body: &Body,
        limits: Option<&VolumeLimits>,
    ) -> Result<(), SkipReason> {
        let target = &op.header.target;
        let moves_to = match body {
            Body::Place { parent, .. } => Some(parent),
            Body::Create { parent, .. } if self.entries.contains_key(target) => Some(parent),
            _ => None,
        };
        if let Some(parent) = moves_to {
            if parent == target || self.is_under(parent, target) {
                return Err(SkipReason::Cycle);
            }
        }
        if let Some(limits) = limits {
            if self.limit_hit(op, limits).is_some() {
                return Err(SkipReason::Limit);
            }
        }
        if matches!(body, Body::Purge) && self.children(target).next().is_some() {
            return Err(SkipReason::NotEmpty);
        }
        Ok(())
    }

    /// The limit a `Create` or `Place` would pass, if any (R7; the kernel's admission check).
    pub fn limit_hit(&self, op: &Op, limits: &VolumeLimits) -> Option<LimitHit> {
        let target = &op.header.target;
        let (parent, new) = match op.body.as_ref()? {
            Body::Create { parent, .. } => (parent, !self.entries.contains_key(target)),
            Body::Place { parent, .. } => (parent, false),
            _ => return None,
        };
        let below = if new { 0 } else { self.height(target) };
        if self.depth(parent) + 1 + below > usize::from(limits.max_depth) {
            return Some(LimitHit::Depth);
        }
        if new && self.entries.len() >= limits.max_entries as usize {
            return Some(LimitHit::Entries);
        }
        if new
            && self.children.get(parent).map_or(0, |c| c.len())
                >= limits.max_entries_per_dir as usize
        {
            return Some(LimitHit::EntriesPerDir);
        }
        None
    }

    /// How many levels lie below `id` (0 for a file or an empty directory).
    pub fn height(&self, id: &InodeId) -> usize {
        self.children(id)
            .map(|c| 1 + self.height(c))
            .max()
            .unwrap_or(0)
    }
}

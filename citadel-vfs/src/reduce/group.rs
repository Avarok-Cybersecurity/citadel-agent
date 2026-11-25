//! Group volumes (`vfs-group.md` §1, §2): positions `(seq, index)`; the member set and the
//! owner (the administrator) are those after every membership event with a smaller `seq`.

use super::apply::Applied;
use super::{FoldHalt, Judge, Position, SkipReason, Tree};
use crate::authorize::{RoleMask, VolumeCtx};
use crate::limits::PEER_RETENTION_MS;
use crate::op::Op;
use crate::principal::Principal;
use crate::volume::{VolumeId, VolumeKind};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupEventKind {
    Joined,
    Left,
    /// Ownership passed to `cid`.
    Owner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupEvent {
    pub seq: u64,
    pub kind: GroupEventKind,
    pub cid: u64,
}

#[derive(Debug, Clone)]
pub struct GroupFold {
    tree: Tree,
    members: BTreeSet<u64>,
    owner: u64,
    last: Option<(u64, u32)>,
}

impl GroupFold {
    /// The volume as of before its first position: `members` and `owner` then (the first owner
    /// is the creator). `None` unless the volume is a Group volume.
    pub fn new(
        volume: VolumeId,
        members: impl IntoIterator<Item = u64>,
        owner: u64,
    ) -> Option<Self> {
        (volume.kind() == VolumeKind::Group).then(|| Self {
            tree: Tree::for_volume(volume),
            members: members.into_iter().collect(),
            owner,
            last: None,
        })
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    pub fn is_member(&self, cid: u64) -> bool {
        self.members.contains(&cid)
    }

    pub fn owner(&self) -> u64 {
        self.owner
    }

    pub(super) fn advance(&mut self, at: (u64, u32)) -> Result<(), FoldHalt> {
        if self.last.is_some_and(|last| at <= last) {
            return Err(FoldHalt::OutOfOrder);
        }
        self.last = Some(at);
        Ok(())
    }

    pub fn event(&mut self, event: GroupEvent) -> Result<(), FoldHalt> {
        self.advance((event.seq, u32::MAX))?;
        match event.kind {
            GroupEventKind::Joined => {
                self.members.insert(event.cid);
            }
            GroupEventKind::Left => {
                self.members.remove(&event.cid);
            }
            GroupEventKind::Owner => self.owner = event.cid,
        }
        Ok(())
    }

    /// Whether the carrier the relay stamped may carry this author's op: a member's op by its
    /// author; anyone else's by any member.
    pub fn carrier_ok(&self, author: &Principal, carrier: u64) -> bool {
        match author {
            Principal::Member { cid } if self.is_member(*cid) => *cid == carrier,
            _ => self.is_member(carrier),
        }
    }

    /// Fold a verified op at `(seq, index)`.
    pub fn apply(
        &mut self,
        seq: u64,
        index: u32,
        op: &Op,
    ) -> Result<Result<Applied, SkipReason>, FoldHalt> {
        self.advance((seq, index))?;
        Ok(self.judge_and_apply(seq, index, op))
    }

    /// Record a skip decided before the reducer, at its position.
    pub fn skip(
        &mut self,
        seq: u64,
        index: u32,
        op: &Op,
        reason: SkipReason,
    ) -> Result<(), FoldHalt> {
        self.advance((seq, index))?;
        self.tree.record_skip(op, reason);
        Ok(())
    }

    fn judge_and_apply(&mut self, seq: u64, index: u32, op: &Op) -> Result<Applied, SkipReason> {
        let members = self.members.clone();
        let owner = self.owner;
        let is_member = |cid: u64| members.contains(&cid);
        let role = |cid: u64| RoleMask::peer(cid == owner);
        let ctx = VolumeCtx {
            kind: VolumeKind::Group,
            is_member: &is_member,
            role: &role,
            retention_ms: PEER_RETENTION_MS,
            now_ms: op.header.at_ms,
        };
        self.tree
            .apply(Position::Group { seq, index }, op, &Judge::Every(&ctx))
    }
}

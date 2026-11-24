//! Folding one op at its position: judge it, then apply it or record why it was skipped.

use super::{Judge, Position, SkipReason, Skipped, Tree};
use crate::authorize::authorize;
use crate::limits::{VolumeLimits, PEER_VOLUME_LIMITS};
use crate::op::Op;
use crate::principal::Principal;

/// What folding an op did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    Yes,
    /// The same op (one `op_id`, one `op_hash`) was already folded: R2, one op.
    AlreadyHeld,
}

impl Tree {
    /// The limits that are reducer rules here (R7): the shared constant in volumes without a
    /// kernel; none where the kernel refuses at admission.
    pub fn reducer_limits(&self) -> Option<&'static VolumeLimits> {
        (!self.volume.has_kernel()).then_some(&PEER_VOLUME_LIMITS)
    }

    /// Fold `op` at `position` (positions must be given in order). A skip is recorded in
    /// `skipped()` and returned.
    pub fn apply(
        &mut self,
        position: Position,
        op: &Op,
        judge: &Judge,
    ) -> Result<Applied, SkipReason> {
        if let Some(hash) = self.seen.get(&op.header.op_id) {
            if *hash == op.op_hash {
                return Ok(Applied::AlreadyHeld);
            }
            return Err(self.skip(op, SkipReason::Duplicate));
        }
        if let Err(reason) = self.check(op, judge) {
            if op.body.is_none() {
                self.redacted.push((op.header.op_id, op.header.target));
            }
            return Err(self.skip(op, reason));
        }
        self.seen.insert(op.header.op_id, op.op_hash);
        self.effects(position, op, Some(judge.ctx()));
        Ok(Applied::Yes)
    }

    /// Whether `op` would apply now, changing nothing: redaction, existence (R4, R5, R9),
    /// `authorize` as `judge` says (R10), then cycles, limits and emptiness (R6, R7, R9).
    pub fn check(&self, op: &Op, judge: &Judge) -> Result<(), SkipReason> {
        let Some(body) = &op.body else {
            return Err(SkipReason::Purged);
        };
        self.existence(op, body)?;
        let judged = match judge {
            Judge::Every(_) => true,
            Judge::ServerOnly(_) => op.header.author == Principal::Server,
        };
        if judged {
            authorize(self, judge.ctx(), op).map_err(SkipReason::Denied)?;
        }
        self.shape(op, body, self.reducer_limits())
    }

    pub(super) fn skip(&mut self, op: &Op, reason: SkipReason) -> SkipReason {
        self.seen.entry(op.header.op_id).or_insert(op.op_hash);
        self.skipped.push(Skipped {
            op_id: op.header.op_id,
            op_hash: op.op_hash,
            reason: reason.clone(),
        });
        reason
    }

    /// Record a skip decided before the reducer (a group's signature checks, a pair's cut).
    pub fn record_skip(&mut self, op: &Op, reason: SkipReason) {
        self.skip(op, reason);
    }

    /// Redacted ops whose target no applied `Purge` removed: each is `Invalid`
    /// (`vfs-ops.md` §5), and a reader holding one is `Unverified(BadRedaction)`.
    pub fn bad_redactions(&self) -> Vec<[u8; 16]> {
        self.redacted
            .iter()
            .filter(|(_, target)| !self.purged.contains(target))
            .map(|(op_id, _)| *op_id)
            .collect()
    }
}

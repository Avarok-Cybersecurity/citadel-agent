//! Judging a carried `SignedOp` at its group position (`vfs-group.md` §2), in the table's order:
//! structure, carrier, the key bound by the log, the signature, duplicate, the author's chain;
//! then the reducer. A key not yet known is a wait, never a skip.

use super::apply::Applied;
use super::{FoldHalt, GroupEvent, GroupFold, SkipReason, Tree};
use crate::hash::sha3_256;
use crate::model::{Admitted, AdmittedKey};
use crate::op::{check_alone, HeaderError, Op, OpError, SignedOp};
use crate::principal::Principal;
use citadel_identity::MlDsa65Pk;
use std::collections::BTreeMap;

/// The public key behind a fingerprint, if the reader holds it yet.
pub enum KeyAnswer {
    Have(MlDsa65Pk),
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Folded(Result<Applied, SkipReason>),
    /// Fold no further until this key's public half is held.
    WaitForKey(String),
    Halt(FoldHalt),
}

#[derive(Debug, Clone)]
pub struct GroupVerifier {
    fold: GroupFold,
    /// The fingerprint the log bound to each author.
    bound: BTreeMap<Principal, String>,
    /// Each author's latest verified op at an earlier position: `(op_hash, lamport)`.
    heads: BTreeMap<Principal, ([u8; 32], u64)>,
}

impl GroupVerifier {
    pub fn new(fold: GroupFold) -> Self {
        Self {
            fold,
            bound: BTreeMap::new(),
            heads: BTreeMap::new(),
        }
    }

    pub fn tree(&self) -> &Tree {
        self.fold.tree()
    }

    pub fn event(&mut self, event: GroupEvent) -> Result<(), FoldHalt> {
        self.fold.event(event)
    }

    /// The op carried at `(seq, index)` by `carrier` (the cid the relay stamped).
    pub fn op(
        &mut self,
        seq: u64,
        index: u32,
        carrier: u64,
        raw: &[u8],
        keys: &dyn Fn(&str) -> KeyAnswer,
    ) -> Verdict {
        let undecodable = |fold: &mut GroupFold| {
            let stub = stub(raw);
            match fold.skip(seq, index, &stub, SkipReason::Invalid) {
                Ok(()) => Verdict::Folded(Err(SkipReason::Invalid)),
                Err(halt) => Verdict::Halt(halt),
            }
        };
        let Ok(signed) = SignedOp::decode(raw) else {
            return undecodable(&mut self.fold);
        };
        let op = match signed.op() {
            Ok(op) => op,
            Err(OpError::Header(HeaderError::UpdateNeeded)) => {
                return Verdict::Halt(FoldHalt::UpdateNeeded)
            }
            Err(_) => return undecodable(&mut self.fold),
        };
        let volume = self.fold.tree().volume.clone();
        let author = op.header.author.clone();
        let early = if check_alone(&op, &volume).is_err() {
            Some(SkipReason::Invalid)
        } else if !self.fold.carrier_ok(&author, carrier) {
            Some(SkipReason::NotAuthor)
        } else if self
            .expected_key(&author)
            .is_some_and(|fp| fp != signed.key)
        {
            Some(SkipReason::KeyChanged)
        } else {
            None
        };
        if let Some(reason) = early {
            return self.skip(seq, index, &op, reason);
        }
        let pk = match keys(&signed.key) {
            KeyAnswer::Have(pk) => pk,
            KeyAnswer::Missing => return Verdict::WaitForKey(signed.key.clone()),
        };
        if signed.verify(&pk).is_err() {
            return self.skip(seq, index, &op, SkipReason::BadSignature);
        }
        match self.fold.tree().seen.get(&op.header.op_id) {
            Some(held) if *held == op.op_hash => return Verdict::Folded(Ok(Applied::AlreadyHeld)),
            _ => {}
        }
        let head = self.heads.get(&author).copied();
        let chained = op.header.prev == head.map(|(hash, _)| hash);
        let rising = head.is_none_or(|(_, lamport)| op.header.lamport > lamport);
        if self
            .fold
            .tree()
            .seen
            .get(&op.header.op_id)
            .is_some_and(|h| *h != op.op_hash)
        {
            self.heads.insert(author, (op.op_hash, op.header.lamport));
            return self.skip(seq, index, &op, SkipReason::Duplicate);
        }
        self.heads
            .insert(author.clone(), (op.op_hash, op.header.lamport));
        if !chained {
            return self.skip(seq, index, &op, SkipReason::StalePrev);
        }
        if !rising {
            return self.skip(seq, index, &op, SkipReason::Invalid);
        }
        let verdict = self.fold.apply(seq, index, &op);
        if let Ok(Ok(Applied::Yes)) = verdict {
            self.bound.entry(author).or_insert(signed.key);
        }
        match verdict {
            Ok(folded) => Verdict::Folded(folded),
            Err(halt) => Verdict::Halt(halt),
        }
    }

    fn skip(&mut self, seq: u64, index: u32, op: &Op, reason: SkipReason) -> Verdict {
        match self.fold.skip(seq, index, op, reason.clone()) {
            Ok(()) => Verdict::Folded(Err(reason)),
            Err(halt) => Verdict::Halt(halt),
        }
    }

    /// The fingerprint an author's op must be signed under: a link guest's own; a member's or
    /// kind A outsider's as the log bound it (by its first applied op, or the `Admit` naming it).
    fn expected_key(&self, author: &Principal) -> Option<String> {
        match author {
            Principal::LinkGuest { key_fp, .. } => Some(key_fp.clone()),
            Principal::Member { cid } => self.bound.get(author).cloned().or_else(|| {
                let key = AdmittedKey::Member { cid: *cid };
                self.fold.tree().entries().find_map(|e| {
                    e.admissions.get(&key).and_then(|r| match &r.value {
                        Some(a) => match &a.who {
                            Admitted::Member { id_fp, .. } => Some(id_fp.clone()),
                            Admitted::Link { .. } => None,
                        },
                        None => None,
                    })
                })
            }),
            Principal::Server => None,
        }
    }
}

/// A placeholder for bytes that decode to no op: its hash is the bytes' hash.
fn stub(raw: &[u8]) -> Op {
    let hash = sha3_256(&[raw]);
    let mut op_id = [0u8; 16];
    op_id.copy_from_slice(&hash[..16]);
    Op {
        header: crate::op::Header {
            volume: String::new(),
            op_id,
            author: Principal::Server,
            lamport: 1,
            prev: None,
            kind: crate::op::OpKind::Remove,
            target: crate::inode::InodeId([0; 16]),
            body_hash: [0; 32],
            at_ms: 0,
        },
        body: None,
        op_hash: hash,
    }
}

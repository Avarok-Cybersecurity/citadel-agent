//! The helper conditions of `vfs-authorize.md` §2: `bit`, `reach`, `dirw` and admission.

use super::{Class, Denied, VolumeCtx};
use crate::inode::InodeId;
use crate::model::{AdmittedKey, Bit, Triple};
use crate::principal::Principal;
use crate::reduce::{Admission, Tree};
use crate::volume::VolumeKind;

/// Who acts, once classified: a member (with its roles) or an admitted outsider.
#[derive(Clone, Copy)]
pub(super) enum Who<'t> {
    Member {
        cid: u64,
    },
    /// `root` is its admission root for the entry being judged.
    Outsider {
        root: InodeId,
        admission: &'t Admission,
    },
}

pub(super) fn member_class(tree: &Tree, ctx: &VolumeCtx, cid: u64, entry: &InodeId) -> Class {
    if ctx.kind == VolumeKind::Pair {
        return Class::Owner;
    }
    if (ctx.role)(cid).manage_files {
        return Class::Administrator;
    }
    match tree.owner(entry) {
        Some(Principal::Member { cid: owner }) if *owner == cid => Class::Owner,
        _ => Class::Member,
    }
}

/// `bit(actor, E, b)`.
pub(super) fn bit(tree: &Tree, ctx: &VolumeCtx, who: Who, entry: &InodeId, b: Bit) -> bool {
    let Some(mode) = tree.mode(entry) else {
        return false;
    };
    match who {
        Who::Member { cid } => match member_class(tree, ctx, cid, entry) {
            Class::Administrator => true,
            Class::Owner if ctx.kind == VolumeKind::Pair => true,
            Class::Owner => mode.has(Triple::Owner, b),
            _ => mode.has(Triple::Members, b),
        },
        Who::Outsider { .. } => mode.has(Triple::Outsiders, b),
    }
}

fn need(ok: bool, b: Bit, at: &InodeId) -> Result<(), Denied> {
    ok.then_some(()).ok_or(Denied::Mode { need: b, at: *at })
}

/// `reach(actor, E)`: `x` on every ancestor from the root (an outsider: from its admission root).
pub(super) fn reach(tree: &Tree, ctx: &VolumeCtx, who: Who, entry: &InodeId) -> Result<(), Denied> {
    let chain = tree.ancestors(entry);
    let from = match who {
        Who::Member { .. } => 0,
        Who::Outsider { root, .. } => match chain.iter().position(|a| *a == root) {
            Some(at) => at,
            None => chain.len(),
        },
    };
    for ancestor in &chain[from..] {
        need(bit(tree, ctx, who, ancestor, Bit::X), Bit::X, ancestor)?;
    }
    Ok(())
}

/// `dirw(actor, P)`: `reach(P)`, then `w` and `x` on `P`.
pub(super) fn dirw(tree: &Tree, ctx: &VolumeCtx, who: Who, dir: &InodeId) -> Result<(), Denied> {
    reach(tree, ctx, who, dir)?;
    need(bit(tree, ctx, who, dir, Bit::W), Bit::W, dir)?;
    need(bit(tree, ctx, who, dir, Bit::X), Bit::X, dir)
}

/// Whether the admission `(key, admission)` admits `actor` at `now_ms`.
pub(super) fn admits(
    key: &AdmittedKey,
    admission: &Admission,
    actor: &Principal,
    now_ms: u64,
) -> bool {
    if !admission.live_at(now_ms) {
        return false;
    }
    match (key, actor) {
        (AdmittedKey::Member { cid }, Principal::Member { cid: who }) => cid == who,
        (
            AdmittedKey::Link { link_id },
            Principal::LinkGuest {
                link_id: own,
                key_fp,
            },
        ) => {
            link_id == own
                && admission.claims.iter().any(|k| k == key_fp)
                && !admission.handed_over.contains(key_fp)
        }
        _ => false,
    }
}

/// Every admission root of `actor` for `entry`, nearest first: `entry` itself, then upward.
pub(super) fn admission_roots<'t>(
    tree: &'t Tree,
    actor: &Principal,
    entry: &InodeId,
    now_ms: u64,
) -> Vec<(InodeId, &'t Admission)> {
    let mut chain = tree.ancestors(entry);
    chain.push(*entry);
    chain
        .iter()
        .rev()
        .flat_map(|at| {
            tree.admissions_on(at)
                .filter(|(key, admission)| admits(key, admission, actor, now_ms))
                .map(move |(_, admission)| (*at, admission))
        })
        .collect()
}

/// The nearest admission root of `actor` for `entry`.
pub(super) fn admitted_at<'t>(
    tree: &'t Tree,
    actor: &Principal,
    entry: &InodeId,
    now_ms: u64,
) -> Option<Who<'t>> {
    admission_roots(tree, actor, entry, now_ms)
        .into_iter()
        .next()
        .map(|(root, admission)| Who::Outsider { root, admission })
}

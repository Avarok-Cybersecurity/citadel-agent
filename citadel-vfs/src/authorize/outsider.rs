//! The Outsider column (`vfs-authorize.md` §4.0, `vfs-outsiders.md` §2–§4): admitted to a
//! subtree, acting by the outsiders triple, owning nothing, within its admission's caps.

use super::actor::{admission_roots, admitted_at, bit, dirw, reach, Who};
use super::member::target_state;
use super::rules::{section3, Author};
use super::{Cap, Denied, VolumeCtx};
use crate::inode::InodeId;
use crate::limits::RATE_WINDOW_MS;
use crate::model::{Admitted, AdmittedKey, Bit, ClaimWhat};
use crate::op::{verify_claim, Body, Op};
use crate::principal::Principal;
use crate::reduce::{Admission, Tree};
use crate::volume::VolumeKind;
use citadel_identity::{fingerprint, MlDsa65Pk};

pub(super) fn outsider(
    tree: &Tree,
    ctx: &VolumeCtx,
    actor: &Principal,
    op: &Op,
    body: &Body,
) -> Result<(), Denied> {
    if ctx.kind == VolumeKind::Device {
        return Err(Denied::NoOutsidersHere);
    }
    let target = op.header.target;
    if let Body::Claim { link_id, what } = body {
        return claim(tree, ctx, actor, op, link_id, what);
    }
    let now = ctx.now_ms;
    let parent = tree.parent(&target).unwrap_or(tree.root);
    let judged = match body {
        Body::Create { parent, .. } => admitted_at(tree, actor, parent, now),
        Body::Place { parent: new, .. } => common_root(tree, actor, &parent, new, now),
        Body::Remove | Body::Restore => admitted_at(tree, actor, &parent, now),
        _ => admitted_at(tree, actor, &target, now),
    };
    let who = judged.ok_or(Denied::NotAdmitted)?;
    if !op.header.kind.outsider_may() {
        return Err(Denied::OutsiderNot);
    }
    target_state(tree, op, body)?;
    match body {
        Body::Create { parent, .. } => dirw(tree, ctx, who, parent)?,
        Body::Place { parent: new, .. } => {
            dirw(tree, ctx, who, &parent)?;
            dirw(tree, ctx, who, new)?;
        }
        Body::SetPointee { .. } => {
            reach(tree, ctx, who, &target)?;
            if !bit(tree, ctx, who, &target, Bit::W) {
                return Err(Denied::Mode {
                    need: Bit::W,
                    at: target,
                });
            }
        }
        _ => dirw(tree, ctx, who, &parent)?,
    }
    let Who::Outsider { admission, .. } = who else {
        return Err(Denied::NotAdmitted);
    };
    section3(
        tree,
        ctx,
        op,
        body,
        Author::Outsider {
            admitter: &admission.by,
        },
    )?;
    caps(tree, admission, op, body, now)
}

/// The admission an applied outsider op is charged to: its admission root and key.
pub(crate) fn admission_charged(
    tree: &Tree,
    ctx: &VolumeCtx,
    op: &Op,
) -> Option<(InodeId, AdmittedKey)> {
    let actor = &op.header.author;
    let outsider = match actor {
        Principal::Member { cid } => !(ctx.is_member)(*cid),
        Principal::LinkGuest { .. } => true,
        Principal::Server => false,
    };
    if !outsider {
        return None;
    }
    let target = op.header.target;
    let parent = tree.parent(&target).unwrap_or(tree.root);
    let (root, admission) = match op.body.as_ref()? {
        Body::Create { parent, .. } => admission_roots(tree, actor, parent, ctx.now_ms)
            .into_iter()
            .next(),
        Body::Place { parent: new, .. } => admission_roots(tree, actor, &parent, ctx.now_ms)
            .into_iter()
            .find(|(root, _)| tree.is_under(new, root)),
        Body::Remove => admission_roots(tree, actor, &parent, ctx.now_ms)
            .into_iter()
            .next(),
        Body::SetPointee { .. } => admission_roots(tree, actor, &target, ctx.now_ms)
            .into_iter()
            .next(),
        _ => None,
    }?;
    Some((root, admission.who.key()))
}

/// For a `Place`: the nearest admission root above (or at) the old parent that is also above
/// (or at) the new one.
fn common_root<'t>(
    tree: &'t Tree,
    actor: &Principal,
    old: &InodeId,
    new: &InodeId,
    now: u64,
) -> Option<Who<'t>> {
    admission_roots(tree, actor, old, now)
        .into_iter()
        .find(|(root, _)| tree.is_under(new, root))
        .map(|(root, admission)| Who::Outsider { root, admission })
}

/// `AdmissionCap`: bytes written, entries created, ops in the last minute.
fn caps(tree: &Tree, admission: &Admission, op: &Op, body: &Body, now: u64) -> Result<(), Denied> {
    let (caps, used) = (&admission.caps, &admission.used);
    let bytes = match body {
        Body::Create { pointee, .. } | Body::SetPointee { pointee } => pointee.byte_len(),
        _ => 0,
    };
    if bytes > 0 && used.bytes.saturating_add(bytes) > caps.max_bytes {
        return Err(Denied::AdmissionCap(Cap::Bytes));
    }
    let new_entry = matches!(body, Body::Create { .. }) && !tree.exists(&op.header.target);
    if new_entry && used.entries.saturating_add(1) > caps.max_entries {
        return Err(Denied::AdmissionCap(Cap::Entries));
    }
    let recent = used
        .recent_ms
        .iter()
        .filter(|at| at.saturating_add(RATE_WINDOW_MS) > now)
        .count() as u64;
    if recent >= caps.max_ops_per_min {
        return Err(Denied::AdmissionCap(Cap::Rate));
    }
    Ok(())
}

/// `Claim`: by a guest of that link, on the entry holding the link's live admission; a first
/// claim needs a claim left and the link key's signature; a handover needs an applied claim.
fn claim(
    tree: &Tree,
    ctx: &VolumeCtx,
    actor: &Principal,
    op: &Op,
    link_id: &[u8; 16],
    what: &ClaimWhat,
) -> Result<(), Denied> {
    let target = op.header.target;
    let Principal::LinkGuest {
        link_id: own,
        key_fp,
    } = actor
    else {
        return match admitted_at(tree, actor, &target, ctx.now_ms) {
            Some(_) => Err(Denied::OutsiderNot),
            None => Err(Denied::NotAdmitted),
        };
    };
    if own != link_id {
        return Err(Denied::OutsiderNot);
    }
    let key = AdmittedKey::Link { link_id: *link_id };
    let admission = tree
        .admissions_on(&target)
        .find(|(k, a)| **k == key && a.live_at(ctx.now_ms))
        .map(|(_, a)| a)
        .ok_or(Denied::NotAdmitted)?;
    let claimed = admission.claims.iter().any(|k| k == key_fp);
    match what {
        ClaimWhat::Handover { .. } => (claimed && !admission.handed_over.contains(key_fp))
            .then_some(())
            .ok_or(Denied::NotAdmitted),
        ClaimWhat::Key { .. } if claimed => Ok(()),
        ClaimWhat::Key { key_pk, link_sig } => {
            let Admitted::Link {
                link_pk,
                max_claims,
                ..
            } = &admission.who
            else {
                return Err(Denied::NotAdmitted);
            };
            if admission.claims.len() as u64 >= *max_claims {
                return Err(Denied::ClaimsExhausted);
            }
            let key_pk = MlDsa65Pk::from_bytes(key_pk).map_err(|_| Denied::BadClaim)?;
            let link_pk = MlDsa65Pk::from_bytes(link_pk).map_err(|_| Denied::BadClaim)?;
            if fingerprint(&key_pk) != *key_fp {
                return Err(Denied::BadClaim);
            }
            verify_claim(&link_pk, &op.header.volume, &target.0, &key_pk, link_sig)
                .map_err(|_| Denied::BadClaim)
        }
    }
}

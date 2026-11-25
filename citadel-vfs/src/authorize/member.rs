//! The Server and member columns of `vfs-authorize.md` §4 (4.1–4.5).

use super::actor::{bit, dirw, member_class, reach, Who};
use super::rules::{section3, Author};
use super::{Class, Denied, RoleNeed, VolumeCtx};
use crate::model::{Bit, EntryKind, Pointee};
use crate::op::{Body, Op};
use crate::principal::Principal;
use crate::reduce::Tree;
use crate::volume::VolumeKind;

pub(super) fn server(tree: &Tree, ctx: &VolumeCtx, op: &Op, body: &Body) -> Result<(), Denied> {
    if matches!(
        ctx.kind,
        VolumeKind::Pair | VolumeKind::Device | VolumeKind::Group
    ) {
        return Err(Denied::NoServerHere);
    }
    if !matches!(body, Body::Create { .. } | Body::Purge) {
        return Err(Denied::ServerOpKind);
    }
    target_state(tree, op, body)?;
    section3(tree, ctx, op, body, Author::Server)
}

pub(super) fn member(
    tree: &Tree,
    ctx: &VolumeCtx,
    cid: u64,
    op: &Op,
    body: &Body,
) -> Result<(), Denied> {
    let roles = (ctx.role)(cid);
    if !roles.view {
        return Err(Denied::Role {
            need: RoleNeed::View,
        });
    }
    let gated = matches!(ctx.kind, VolumeKind::Node | VolumeKind::Personal);
    if gated && op.header.kind.needs_upload() && !roles.upload {
        return Err(Denied::Role {
            need: RoleNeed::Upload,
        });
    }
    target_state(tree, op, body)?;
    cell(tree, ctx, cid, op, body)?;
    section3(tree, ctx, op, body, Author::Member(cid))
}

/// Step 5: the target's kind and state.
pub(super) fn target_state(tree: &Tree, op: &Op, body: &Body) -> Result<(), Denied> {
    let target = &op.header.target;
    match body {
        Body::SetPointee { .. } => match tree.entry(target) {
            Some(e) if e.kind == EntryKind::File => Ok(()),
            _ => Err(Denied::NotAFile),
        },
        Body::Restore => match tree.entry(target).and_then(|e| e.removal()) {
            Some(_) => Ok(()),
            None => Err(Denied::NotRemoved),
        },
        Body::Purge if !tree.is_removed(target) => Err(Denied::NotRemoved),
        _ => Ok(()),
    }
}

/// Step 6: the class and mode cell.
fn cell(tree: &Tree, ctx: &VolumeCtx, cid: u64, op: &Op, body: &Body) -> Result<(), Denied> {
    let who = Who::Member { cid };
    let target = op.header.target;
    let parent = tree.parent(&target).unwrap_or(tree.root);
    let class = member_class(tree, ctx, cid, &target);
    match body {
        Body::Create { parent, .. } => dirw(tree, ctx, who, parent),
        Body::Place { parent: new, .. } => {
            dirw(tree, ctx, who, &parent)?;
            dirw(tree, ctx, who, new)
        }
        Body::SetPointee { .. } => {
            reach(tree, ctx, who, &target)?;
            bit(tree, ctx, who, &target, Bit::W)
                .then_some(())
                .ok_or(Denied::Mode {
                    need: Bit::W,
                    at: target,
                })
        }
        Body::Remove if is_host(tree, cid, &target) => Ok(()),
        Body::Remove | Body::Restore => dirw(tree, ctx, who, &parent),
        Body::Purge => match class {
            Class::Administrator | Class::Owner => Ok(()),
            _ => {
                let removed_by_me = tree
                    .removal_star(&target)
                    .is_some_and(|r| r.by == Principal::member(cid));
                (removed_by_me && dirw(tree, ctx, who, &parent).is_ok())
                    .then_some(())
                    .ok_or(Denied::NotOwner)
            }
        },
        Body::Chmod { .. } | Body::Chown { .. } | Body::Admit { .. } | Body::Dismiss { .. } => {
            match class {
                Class::Administrator | Class::Owner => Ok(()),
                _ => Err(Denied::NotOwner),
            }
        }
        Body::Claim { .. } => Err(Denied::OutsiderNot),
    }
}

/// The host of an entry's `Revfs` pointee may always remove it.
fn is_host(tree: &Tree, cid: u64, target: &crate::InodeId) -> bool {
    matches!(
        tree.entry(target).map(|e| &e.pointee.value),
        Some(Pointee::Revfs { host: Principal::Member { cid: host }, .. }) if *host == cid
    )
}

//! Reading (`vfs-authorize.md` §5): listing, an entry's metadata, a file's bytes.

use super::actor::{admitted_at, bit, reach, Who};
use super::{Denied, RoleNeed, VolumeCtx};
use crate::inode::InodeId;
use crate::model::{Bit, EntryKind, Pointee};
use crate::principal::Principal;
use crate::reduce::Tree;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Read {
    Metadata,
    List,
    Bytes,
}

/// An entry's metadata: member, `role.view`, `reach(entry)`.
pub fn may_see(
    tree: &Tree,
    ctx: &VolumeCtx,
    actor: &Principal,
    entry: InodeId,
) -> Result<(), Denied> {
    check(tree, ctx, actor, entry, Read::Metadata)
}

/// Listing a directory: member, `role.view`, `reach(dir)`, `bit(dir, R)`.
pub fn may_list(
    tree: &Tree,
    ctx: &VolumeCtx,
    actor: &Principal,
    dir: InodeId,
) -> Result<(), Denied> {
    check(tree, ctx, actor, dir, Read::List)
}

/// A file's bytes: member, `role.view`, `reach`, `bit(R)`, `role.download`, and for a `Revfs`
/// pointee the actor is its owner.
pub fn may_read_bytes(
    tree: &Tree,
    ctx: &VolumeCtx,
    actor: &Principal,
    file: InodeId,
) -> Result<(), Denied> {
    if tree.entry(&file).is_some_and(|e| e.kind == EntryKind::Dir) {
        return Err(Denied::NotAFile);
    }
    check(tree, ctx, actor, file, Read::Bytes)
}

fn check(
    tree: &Tree,
    ctx: &VolumeCtx,
    actor: &Principal,
    at: InodeId,
    read: Read,
) -> Result<(), Denied> {
    let revfs_owner = match tree.entry(&at).map(|e| &e.pointee.value) {
        Some(Pointee::Revfs { owner, .. }) => Some(*owner),
        _ => None,
    };
    match actor {
        Principal::Server => Err(Denied::ServerOpKind),
        Principal::Member { cid } if (ctx.is_member)(*cid) => {
            let roles = (ctx.role)(*cid);
            if !roles.view {
                return Err(Denied::Role {
                    need: RoleNeed::View,
                });
            }
            let who = Who::Member { cid: *cid };
            reach(tree, ctx, who, &at)?;
            if read != Read::Metadata && !bit(tree, ctx, who, &at, Bit::R) {
                return Err(Denied::Mode { need: Bit::R, at });
            }
            if read == Read::Bytes {
                if !roles.download {
                    return Err(Denied::Role {
                        need: RoleNeed::Download,
                    });
                }
                if revfs_owner.is_some_and(|owner| owner != *cid) {
                    return Err(Denied::RevfsOwnerOnly);
                }
            }
            Ok(())
        }
        outsider => {
            let who = admitted_at(tree, outsider, &at, ctx.now_ms).ok_or(Denied::NotAdmitted)?;
            let Who::Outsider { root, .. } = who else {
                return Err(Denied::NotAdmitted);
            };
            let mut chain = tree.ancestors(&at);
            chain.push(at);
            let from = chain.iter().position(|a| *a == root).unwrap_or(0);
            let removed = chain[from..]
                .iter()
                .any(|a| tree.entry(a).is_some_and(|e| e.removal().is_some()));
            if removed {
                return Err(Denied::NotAdmitted);
            }
            reach(tree, ctx, who, &at)?;
            if read != Read::Metadata && !bit(tree, ctx, who, &at, Bit::R) {
                return Err(Denied::Mode { need: Bit::R, at });
            }
            if read == Read::Bytes && revfs_owner.is_some() {
                return Err(Denied::RevfsOwnerOnly);
            }
            Ok(())
        }
    }
}

//! The rules that hold in every volume kind (`vfs-authorize.md` §3), in the order listed there.

use super::{Denied, RoleNeed, VolumeCtx};
use crate::model::{Admitted, AdmittedKey, Pointee};
use crate::op::{Body, Op};
use crate::principal::Principal;
use crate::reduce::Tree;
use crate::volume::VolumeKind;

/// The author, as §3 distinguishes it.
pub(super) enum Author<'a> {
    Server,
    Member(u64),
    /// An admitted outsider; `admitter` is its admission's `by`.
    Outsider {
        admitter: &'a Principal,
    },
}

impl Author<'_> {
    fn cid(&self) -> Option<u64> {
        match self {
            Self::Member(cid) => Some(*cid),
            _ => None,
        }
    }
}

fn is_member_principal(ctx: &VolumeCtx, p: &Principal) -> bool {
    matches!(p, Principal::Member { cid } if (ctx.is_member)(*cid))
}

/// The pointee an op writes, if it writes one.
fn written(body: &Body) -> Option<&Pointee> {
    match body {
        Body::Create { pointee, .. } | Body::SetPointee { pointee } => Some(pointee),
        _ => None,
    }
}

pub(super) fn section3(
    tree: &Tree,
    ctx: &VolumeCtx,
    op: &Op,
    body: &Body,
    author: Author,
) -> Result<(), Denied> {
    let target = &op.header.target;
    if let Body::Create { owner, .. } = body {
        create_owner(ctx, &author, owner)?;
    }
    if let Some(pointee) = written(body) {
        if let Pointee::Revfs { owner, .. } = pointee {
            if author.cid() != Some(*owner) {
                return Err(Denied::RevfsOwnerOnly);
            }
        }
        if let Some(Pointee::Revfs { owner, .. }) = tree.entry(target).map(|e| &e.pointee.value) {
            if author.cid() != Some(*owner) {
                return Err(Denied::RevfsOwnerOnly);
            }
        }
        if let Pointee::Revfs { owner, host, .. } = pointee {
            revfs_host(ctx, *owner, host)?;
        }
    }
    match body {
        Body::Purge => host_purge(tree, ctx, op, &author),
        Body::Chown { owner } => is_member_principal(ctx, owner)
            .then_some(())
            .ok_or(Denied::NewOwnerNotMember),
        Body::Admit { who, .. } => admit_dismiss(ctx, &author, Some(who), &who.key()),
        Body::Dismiss { who, .. } => admit_dismiss(ctx, &author, None, who),
        _ => Ok(()),
    }
}

/// `Create.owner` is the author; a pair's either member; a server's any member or `Server`
/// (in a Personal volume, the account); an outsider's admitter.
fn create_owner(ctx: &VolumeCtx, author: &Author, owner: &Principal) -> Result<(), Denied> {
    let ok = match author {
        Author::Server => match ctx.kind {
            VolumeKind::Node => *owner == Principal::Server || is_member_principal(ctx, owner),
            _ => is_member_principal(ctx, owner),
        },
        Author::Member(_) if ctx.kind == VolumeKind::Pair => is_member_principal(ctx, owner),
        Author::Member(cid) => *owner == Principal::member(*cid),
        Author::Outsider { admitter } => {
            return (owner == *admitter)
                .then_some(())
                .ok_or(Denied::OwnerNotAdmitter);
        }
    };
    ok.then_some(()).ok_or(Denied::OwnerNotAuthor)
}

/// A new `Revfs` pointee's host: `Server` in Personal and Node; the other member in a Pair; a
/// member other than the owner in a Group; never in a Device volume.
fn revfs_host(ctx: &VolumeCtx, owner: u64, host: &Principal) -> Result<(), Denied> {
    let ok = match ctx.kind {
        VolumeKind::Personal | VolumeKind::Node => *host == Principal::Server,
        VolumeKind::Pair | VolumeKind::Group => {
            matches!(host, Principal::Member { cid } if *cid != owner && (ctx.is_member)(*cid))
        }
        VolumeKind::Device => false,
    };
    ok.then_some(()).ok_or(Denied::RevfsHost)
}

/// A `Revfs` entry is purged by anyone but its owner only once the owner removed it or
/// retention has passed since its removal.
fn host_purge(tree: &Tree, ctx: &VolumeCtx, op: &Op, author: &Author) -> Result<(), Denied> {
    let target = &op.header.target;
    let Some(Pointee::Revfs { owner, .. }) = tree.entry(target).map(|e| &e.pointee.value) else {
        return Ok(());
    };
    if author.cid() == Some(*owner) {
        return Ok(());
    }
    let Some(removal) = tree.removal_star(target) else {
        return Err(Denied::NotRemoved);
    };
    let by_owner = removal.by == Principal::member(*owner);
    let waited = op.header.at_ms >= removal.at_ms.saturating_add(ctx.retention_ms);
    (by_owner || waited)
        .then_some(())
        .ok_or(Denied::HostCannotPurge)
}

/// `Admit` and `Dismiss`, after the class cell: never in a Device volume; kind A names a
/// non-member; a link needs `share_links`.
fn admit_dismiss(
    ctx: &VolumeCtx,
    author: &Author,
    who: Option<&Admitted>,
    key: &AdmittedKey,
) -> Result<(), Denied> {
    if ctx.kind == VolumeKind::Device {
        return Err(Denied::NoOutsidersHere);
    }
    if let Some(Admitted::Member { cid, .. }) = who {
        if (ctx.is_member)(*cid) {
            return Err(Denied::AlreadyMember);
        }
    }
    if let (AdmittedKey::Link { .. }, Some(cid)) = (key, author.cid()) {
        if !(ctx.role)(cid).share_links {
            return Err(Denied::Role {
                need: RoleNeed::ShareLinks,
            });
        }
    }
    Ok(())
}

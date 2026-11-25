//! Structural validity (`vfs-ops.md` §3), checked by every party before anything else. Decoding
//! already holds `v`, the body's shape and hash, names, modes, kinds and field bounds.

use crate::inode::InodeId;
use crate::limits::SECURE_LINK_MAX_TTL_MS;
use crate::model::{Admitted, EntryKind};
use crate::op::{Body, Header, Op};
use crate::principal::Principal;
use crate::volume::{VolumeId, VolumeKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Invalid {
    /// Not the volume it arrived for.
    Volume,
    /// A `Dir` created with a pointee, or a `File` with none; or `SetPointee` to none.
    Pointee,
    TargetIsParent,
    TargetIsRoot,
    /// `lamport` does not rise along the author's own chain.
    Lamport,
    /// A `Claim` not authored by a guest of its own link.
    ClaimAuthor,
    /// A Device volume op that names an outsider.
    DeviceOutsider,
    /// A link admission without an expiry, or longer than `SECURE_LINK_MAX_TTL_MS`.
    AdmitExpiry,
}

/// Everything one op alone shows.
pub fn check_alone(op: &Op, volume: &VolumeId) -> Result<(), Invalid> {
    let header = &op.header;
    if header.volume != volume.to_string() {
        return Err(Invalid::Volume);
    }
    if header.target == InodeId::root(volume) {
        return Err(Invalid::TargetIsRoot);
    }
    if volume.kind() == VolumeKind::Device {
        device_names_no_outsider(header, op.body.as_ref(), volume)?;
    }
    let Some(body) = &op.body else {
        return Ok(());
    };
    match body {
        Body::Create {
            kind,
            parent,
            pointee,
            ..
        } => {
            if *parent == header.target {
                return Err(Invalid::TargetIsParent);
            }
            let shaped = match kind {
                EntryKind::Dir => pointee.is_none(),
                EntryKind::File => !pointee.is_none(),
            };
            shaped.then_some(()).ok_or(Invalid::Pointee)
        }
        Body::Place { parent, .. } if *parent == header.target => Err(Invalid::TargetIsParent),
        Body::SetPointee { pointee } if pointee.is_none() => Err(Invalid::Pointee),
        Body::Claim { link_id, .. } => match &header.author {
            Principal::LinkGuest { link_id: own, .. } if own == link_id => Ok(()),
            _ => Err(Invalid::ClaimAuthor),
        },
        Body::Admit {
            who: Admitted::Link { .. },
            expires_ms,
            ..
        } => {
            let ceiling = header.at_ms.saturating_add(SECURE_LINK_MAX_TTL_MS);
            (*expires_ms != 0 && *expires_ms <= ceiling)
                .then_some(())
                .ok_or(Invalid::AdmitExpiry)
        }
        _ => Ok(()),
    }
}

/// `lamport` rises along the author's chain: `prev` names `previous`.
pub fn check_chain(op: &Op, previous: &Header) -> Result<(), Invalid> {
    (op.header.lamport > previous.lamport)
        .then_some(())
        .ok_or(Invalid::Lamport)
}

fn device_names_no_outsider(
    header: &Header,
    body: Option<&Body>,
    volume: &VolumeId,
) -> Result<(), Invalid> {
    let VolumeId::Device { cid } = volume else {
        return Ok(());
    };
    if header.kind == crate::op::OpKind::Claim {
        return Ok(());
    }
    let outsider_author = match &header.author {
        Principal::Member { cid: author } => author != cid,
        Principal::LinkGuest { .. } => true,
        Principal::Server => false,
    };
    let names_one = matches!(body, Some(Body::Admit { .. } | Body::Dismiss { .. }));
    if outsider_author || names_one {
        return Err(Invalid::DeviceOutsider);
    }
    Ok(())
}

//! Planned entries, emitted parents first and siblings in ascending byte order of their names.

use super::{Migration, OpDraft, SkipReason};
use crate::inode::{InodeId, Namespace};
use crate::limits::{DEFAULT_DIR_MODE, DEFAULT_FILE_MODE};
use crate::model::{EntryKind, Mode, Pointee};
use crate::name::Name;
use crate::op::{Body, OpKind};
use crate::principal::Principal;
use crate::volume::VolumeId;

pub(super) struct Plan {
    pub name: Name,
    pub draft: OpDraft,
    pub children: Vec<Plan>,
}

pub(super) struct Entry<'a> {
    pub volume: &'a VolumeId,
    pub kind: EntryKind,
    pub namespace: Namespace,
    pub key: String,
    pub parent: InodeId,
    pub name: Name,
    pub pointee: Pointee,
    pub owner: Principal,
    pub at_ms: u64,
}

impl Entry<'_> {
    /// The entry's `Create`; `Inconsistent` if its key cannot be framed or its pointee is out
    /// of bounds.
    pub fn plan(self) -> Result<Plan, SkipReason> {
        let target = InodeId::derive(self.volume, self.namespace, &self.key)
            .map_err(|_| SkipReason::Inconsistent)?;
        self.pointee.check().map_err(|_| SkipReason::Inconsistent)?;
        let bits = match self.kind {
            EntryKind::Dir => DEFAULT_DIR_MODE,
            EntryKind::File => DEFAULT_FILE_MODE,
        };
        let mode = Mode::new(bits).ok_or(SkipReason::Inconsistent)?;
        let body = Body::Create {
            kind: self.kind,
            parent: self.parent,
            name: self.name.clone(),
            pointee: self.pointee,
            mode,
            owner: self.owner,
        };
        let draft = OpDraft {
            kind: OpKind::Create,
            target,
            body,
            at_ms: self.at_ms,
            namespace: self.namespace.as_str(),
            key: self.key,
        };
        Ok(Plan {
            name: self.name,
            draft,
            children: Vec::new(),
        })
    }
}

/// A seeded directory at the root (`vfs-model.md` §2, namespace `seed`).
pub(super) fn seed(
    volume: &VolumeId,
    name: &str,
    owner: Principal,
    at_ms: u64,
) -> Result<Plan, SkipReason> {
    Entry {
        volume,
        kind: EntryKind::Dir,
        namespace: Namespace::Seed,
        key: name.to_string(),
        parent: InodeId::root(volume),
        name: Name::new(name).map_err(|_| SkipReason::BadName)?,
        pointee: Pointee::None,
        owner,
        at_ms,
    }
    .plan()
}

/// The pointee with its `mime` dropped when only the `mime` is out of bounds (a legacy type
/// string is advisory).
pub(super) fn tolerant_mime(pointee: Pointee) -> Pointee {
    if pointee.check().is_ok() {
        return pointee;
    }
    match pointee {
        Pointee::Standard { sha256, len, .. } => Pointee::Standard {
            sha256,
            len,
            mime: String::new(),
        },
        Pointee::Revfs {
            owner,
            host,
            virtual_path,
            len,
            ..
        } => Pointee::Revfs {
            owner,
            host,
            virtual_path,
            len,
            mime: String::new(),
        },
        other => other,
    }
}

pub(super) fn emit(mut plans: Vec<Plan>, out: &mut Migration) {
    plans.sort_by(|a, b| a.name.as_str().as_bytes().cmp(b.name.as_str().as_bytes()));
    for plan in plans {
        out.ops.push(plan.draft);
        emit(plan.children, out);
    }
}

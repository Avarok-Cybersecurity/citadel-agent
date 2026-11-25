//! A volume's state after a fold: entries with their registers, purge markers and skips.

use super::{Reg, Skipped};
use crate::inode::InodeId;
use crate::limits::ROOT_MODE;
use crate::model::{Admitted, AdmittedKey, Caps, EntryKind, Mode, Pointee, Stamped};
use crate::name::Name;
use crate::principal::Principal;
use crate::volume::VolumeId;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub parent: InodeId,
    pub name: Name,
}

/// What an outsider admitted here has used (`AdmissionCap`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Usage {
    /// Bytes of every pointee the outsider wrote under this admission.
    pub bytes: u64,
    /// Entries the outsider created under this admission.
    pub entries: u64,
    /// `at_ms` of its most recent ops, for `max_ops_per_min`.
    pub recent_ms: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admission {
    pub who: Admitted,
    pub by: Principal,
    /// 0 = until dismissed.
    pub expires_ms: u64,
    pub caps: Caps,
    /// Applied claims, by key fingerprint.
    pub claims: Vec<String>,
    /// Claimed keys whose access was handed to an identity: refused from then on.
    pub handed_over: BTreeSet<String>,
    pub used: Usage,
}

impl Admission {
    pub fn live_at(&self, now_ms: u64) -> bool {
        self.expires_ms == 0 || now_ms < self.expires_ms
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: InodeId,
    pub kind: EntryKind,
    pub created: Stamped,
    pub place: Reg<Place>,
    pub pointee: Reg<Pointee>,
    pub owner: Reg<Principal>,
    pub mode: Reg<Mode>,
    /// `None` until a `Remove` or `Restore` writes it.
    pub removal: Option<Reg<Option<Stamped>>>,
    pub admissions: BTreeMap<AdmittedKey, Reg<Option<Admission>>>,
}

impl Entry {
    pub fn removal(&self) -> Option<&Stamped> {
        self.removal.as_ref().and_then(|r| r.value.as_ref())
    }
}

/// The derived state (`vfs-model.md` §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryState {
    Removed,
    Revfs,
    Shared,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    pub volume: VolumeId,
    pub root: InodeId,
    pub root_owner: Principal,
    pub root_mode: Mode,
    pub(crate) entries: BTreeMap<InodeId, Entry>,
    pub(crate) children: BTreeMap<InodeId, BTreeSet<InodeId>>,
    pub(crate) purged: BTreeSet<InodeId>,
    pub(crate) skipped: Vec<Skipped>,
    /// Every op folded or skipped at a position: `op_id → op_hash`.
    pub(crate) seen: BTreeMap<[u8; 16], [u8; 32]>,
    /// Redacted ops skipped as `Purged`, by target, to check once the log is folded.
    pub(crate) redacted: Vec<([u8; 16], InodeId)>,
}

impl Tree {
    /// An empty volume: its root, owned as `vfs-model.md` §4 says, at `ROOT_MODE`.
    pub fn new(volume: VolumeId, root_owner: Principal) -> Self {
        let root = InodeId::root(&volume);
        let root_mode = Mode::new(ROOT_MODE).unwrap_or_else(|| unreachable!("0o770 is a mode"));
        Self::with_root(volume, root_owner, root_mode, root)
    }

    /// The root's owner for a volume, where the volume id says it (`None` for Node and Pair,
    /// whose roots are `Server`'s and unused respectively; see `Tree::for_volume`).
    pub fn for_volume(volume: VolumeId) -> Self {
        let owner = match &volume {
            VolumeId::Personal { cid } | VolumeId::Device { cid } => Principal::member(*cid),
            VolumeId::Group { creator, .. } => Principal::member(*creator),
            VolumeId::Pair { lo, .. } => Principal::member(*lo),
            VolumeId::Node { .. } => Principal::Server,
        };
        Self::new(volume, owner)
    }

    pub(crate) fn with_root(
        volume: VolumeId,
        root_owner: Principal,
        root_mode: Mode,
        root: InodeId,
    ) -> Self {
        Self {
            volume,
            root,
            root_owner,
            root_mode,
            entries: BTreeMap::new(),
            children: BTreeMap::new(),
            purged: BTreeSet::new(),
            skipped: Vec::new(),
            seen: BTreeMap::new(),
            redacted: Vec::new(),
        }
    }

    pub fn entry(&self, id: &InodeId) -> Option<&Entry> {
        self.entries.get(id)
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }

    pub fn purged(&self) -> &BTreeSet<InodeId> {
        &self.purged
    }

    pub fn skipped(&self) -> &[Skipped] {
        &self.skipped
    }

    pub fn exists(&self, id: &InodeId) -> bool {
        *id == self.root || self.entries.contains_key(id)
    }

    pub fn is_dir(&self, id: &InodeId) -> bool {
        *id == self.root
            || self
                .entries
                .get(id)
                .is_some_and(|e| e.kind == EntryKind::Dir)
    }

    pub fn parent(&self, id: &InodeId) -> Option<InodeId> {
        self.entries.get(id).map(|e| e.place.value.parent)
    }

    pub fn owner(&self, id: &InodeId) -> Option<&Principal> {
        if *id == self.root {
            return Some(&self.root_owner);
        }
        self.entries.get(id).map(|e| &e.owner.value)
    }

    pub fn mode(&self, id: &InodeId) -> Option<Mode> {
        if *id == self.root {
            return Some(self.root_mode);
        }
        self.entries.get(id).map(|e| e.mode.value)
    }

    pub fn children(&self, id: &InodeId) -> impl Iterator<Item = &InodeId> {
        self.children.get(id).into_iter().flatten()
    }

    /// Ancestors of `id` from the root down, `id` excluded; empty for the root or an unknown id.
    pub fn ancestors(&self, id: &InodeId) -> Vec<InodeId> {
        let mut chain = Vec::new();
        let mut at = self.parent(id);
        while let Some(parent) = at {
            chain.push(parent);
            at = if parent == self.root {
                None
            } else {
                self.parent(&parent)
            };
        }
        chain.reverse();
        chain
    }

    /// The root is depth 0.
    pub fn depth(&self, id: &InodeId) -> usize {
        self.ancestors(id).len()
    }

    /// `removal*(E)`: its own removal, else that of the nearest ancestor with one.
    pub fn removal_star(&self, id: &InodeId) -> Option<&Stamped> {
        let own = self.entries.get(id).and_then(Entry::removal);
        own.or_else(|| {
            self.ancestors(id)
                .iter()
                .rev()
                .find_map(|a| self.entries.get(a).and_then(Entry::removal))
        })
    }

    pub fn is_removed(&self, id: &InodeId) -> bool {
        self.removal_star(id).is_some()
    }

    pub fn state(&self, id: &InodeId) -> Option<EntryState> {
        let entry = self.entries.get(id)?;
        Some(if self.is_removed(id) {
            EntryState::Removed
        } else if matches!(entry.pointee.value, Pointee::Revfs { .. }) {
            EntryState::Revfs
        } else {
            EntryState::Shared
        })
    }

    /// Whether `ancestor` is `id` or above it.
    pub fn is_under(&self, id: &InodeId, ancestor: &InodeId) -> bool {
        id == ancestor || self.ancestors(id).contains(ancestor)
    }

    /// The live admissions recorded on `id` itself.
    pub fn admissions_on(&self, id: &InodeId) -> impl Iterator<Item = (&AdmittedKey, &Admission)> {
        self.entries.get(id).into_iter().flat_map(|e| {
            e.admissions
                .iter()
                .filter_map(|(k, r)| r.value.as_ref().map(|a| (k, a)))
        })
    }
}

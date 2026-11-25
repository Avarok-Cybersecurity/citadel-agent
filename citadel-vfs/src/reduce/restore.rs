//! A tree from its parts: a kernel's snapshot (`vfs-wire.md` §3.3) or the view an outsider is
//! served (`vfs-outsiders.md` §5). The parts must form a tree; nothing is repaired.

use super::{Entry, Tree};
use crate::inode::InodeId;
use crate::model::{EntryKind, Mode};
use crate::principal::Principal;
use crate::volume::VolumeId;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreError {
    /// An entry whose parent is neither the root nor a directory among the parts.
    NoParent(InodeId),
    /// An entry that is its own ancestor.
    Cycle(InodeId),
    /// An id given twice, as the root, or both live and purged.
    Repeated(InodeId),
}

impl Tree {
    pub fn from_parts(
        volume: VolumeId,
        root_owner: Principal,
        root_mode: Mode,
        entries: Vec<Entry>,
        purged: BTreeSet<InodeId>,
    ) -> Result<Self, RestoreError> {
        let root = InodeId::root(&volume);
        let mut tree = Self::with_root(volume, root_owner, root_mode, root);
        for entry in entries {
            let id = entry.id;
            if id == root || purged.contains(&id) || tree.entries.contains_key(&id) {
                return Err(RestoreError::Repeated(id));
            }
            tree.entries.insert(id, entry);
        }
        for entry in tree.entries.values() {
            let parent = entry.place.value.parent;
            let dir = parent == root
                || tree
                    .entries
                    .get(&parent)
                    .is_some_and(|p| p.kind == EntryKind::Dir);
            if !dir {
                return Err(RestoreError::NoParent(entry.id));
            }
        }
        for id in tree.entries.keys() {
            let mut seen = BTreeSet::new();
            let mut at = *id;
            while at != root {
                if !seen.insert(at) {
                    return Err(RestoreError::Cycle(*id));
                }
                at = tree.entries[&at].place.value.parent;
            }
        }
        let links: Vec<(InodeId, InodeId)> = tree
            .entries
            .values()
            .map(|e| (e.place.value.parent, e.id))
            .collect();
        for (parent, id) in links {
            tree.children.entry(parent).or_default().insert(id);
        }
        tree.purged = purged;
        Ok(tree)
    }
}

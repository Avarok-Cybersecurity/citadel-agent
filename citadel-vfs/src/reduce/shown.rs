//! Names shown (`vfs-ops.md` R11): stored names never change; among a directory's entries that
//! are not removed, equal fold keys rank by the position that won their `place`, and the k-th
//! (k >= 2) is shown as `stem (n)ext` with the smallest free `n >= 2`.

use super::Tree;
use crate::inode::InodeId;
use crate::name::fold_key;
use std::collections::{BTreeMap, BTreeSet};

/// `stem (n)ext`: `ext` starts at the last `.` unless that `.` is the first character.
pub fn numbered(name: &str, n: u64) -> String {
    match name.rfind('.') {
        Some(at) if at > 0 => format!("{} ({n}){}", &name[..at], &name[at..]),
        _ => format!("{name} ({n})"),
    }
}

impl Tree {
    /// The shown name of every child of `dir` that is not removed.
    pub fn shown_names(&self, dir: &InodeId) -> BTreeMap<InodeId, String> {
        let mut live: Vec<_> = self
            .children(dir)
            .filter(|id| !self.is_removed(id))
            .filter_map(|id| self.entries.get(id))
            .collect();
        live.sort_by(|a, b| {
            a.place
                .won
                .position
                .cmp(&b.place.won.position)
                .then(a.id.cmp(&b.id))
        });
        let stored: BTreeSet<String> = live.iter().map(|e| e.place.value.name.fold_key()).collect();
        let mut first_of: BTreeSet<String> = BTreeSet::new();
        let mut assigned: BTreeSet<String> = BTreeSet::new();
        let mut shown = BTreeMap::new();
        for entry in live {
            let name = entry.place.value.name.as_str();
            let key = fold_key(name);
            if first_of.insert(key.clone()) {
                assigned.insert(key);
                shown.insert(entry.id, name.to_string());
                continue;
            }
            let mut n = 2u64;
            let candidate = loop {
                let candidate = numbered(name, n);
                let candidate_key = fold_key(&candidate);
                if !stored.contains(&candidate_key) && !assigned.contains(&candidate_key) {
                    assigned.insert(candidate_key);
                    break candidate;
                }
                n += 1;
            };
            shown.insert(entry.id, candidate);
        }
        shown
    }

    /// The shown name of one entry; for a removed entry, its stored name.
    pub fn shown_name(&self, id: &InodeId) -> Option<String> {
        let entry = self.entries.get(id)?;
        if self.is_removed(id) {
            return Some(entry.place.value.name.as_str().to_string());
        }
        self.shown_names(&entry.place.value.parent).remove(id)
    }

    /// The entry a `/`-separated path of shown names reaches from the root (`sdk::fs`, deep links).
    pub fn resolve(&self, path: &str) -> Option<InodeId> {
        let mut at = self.root;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            let names = self.shown_names(&at);
            at = names
                .into_iter()
                .find(|(_, shown)| shown == part)
                .map(|(id, _)| id)?;
        }
        Some(at)
    }

    /// The shown names from the root down to `id`, `id` included (the root itself is empty).
    pub fn path_of(&self, id: &InodeId) -> Option<Vec<String>> {
        let mut chain = self.ancestors(id);
        chain.push(*id);
        chain
            .iter()
            .filter(|a| **a != self.root)
            .map(|a| self.shown_name(a))
            .collect()
    }

    /// Whether `name` collides with a live sibling under `dir` other than `except` (the kernel's
    /// `AlreadyExists { id: false }`).
    pub fn name_collides(&self, dir: &InodeId, name: &str, except: &InodeId) -> bool {
        let key = fold_key(name);
        self.children(dir)
            .filter(|id| *id != except && !self.is_removed(id))
            .filter_map(|id| self.entries.get(id))
            .any(|e| e.place.value.name.fold_key() == key)
    }
}

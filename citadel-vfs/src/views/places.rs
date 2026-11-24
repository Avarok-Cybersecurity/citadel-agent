//! `/Recycle Bin`, `/Shared with me`, and the view an outsider is served.

use crate::inode::InodeId;
use crate::model::{Admitted, Stamped};
use crate::principal::Principal;
use crate::reduce::{Entry, Tree};
use crate::volume::VolumeId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinItem {
    pub id: InodeId,
    pub name: String,
    /// The shown names from the root to the original parent.
    pub parent_path: Vec<String>,
    pub removal: Stamped,
}

/// The top-most removed entries: removed, with no removed ancestor. Every member sees all.
pub fn recycle_bin(tree: &Tree) -> Vec<BinItem> {
    tree.entries()
        .filter_map(|e| {
            let removal = e.removal()?.clone();
            let ancestors = tree.ancestors(&e.id);
            if ancestors
                .iter()
                .any(|a| tree.entry(a).is_some_and(|p| p.removal().is_some()))
            {
                return None;
            }
            let parent = e.place.value.parent;
            let parent_path = if parent == tree.root {
                Vec::new()
            } else {
                tree.path_of(&parent)?
            };
            Some(BinItem {
                id: e.id,
                name: e.place.value.name.as_str().to_string(),
                parent_path,
                removal,
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedItem {
    pub volume: VolumeId,
    pub entry: InodeId,
    pub by: Principal,
    pub expires_ms: u64,
}

/// One item per live admission of `me` (`[1, me, …]`) in a volume `me` is not a member of.
pub fn shared_with_me<'t>(
    trees: impl IntoIterator<Item = &'t Tree>,
    is_member_of: &dyn Fn(&VolumeId) -> bool,
    me: u64,
    now_ms: u64,
) -> Vec<SharedItem> {
    let mut out = Vec::new();
    for tree in trees.into_iter().filter(|t| !is_member_of(&t.volume)) {
        for entry in tree.entries() {
            if tree.is_removed(&entry.id) {
                continue;
            }
            for (_, admission) in tree.admissions_on(&entry.id) {
                let mine = matches!(admission.who, Admitted::Member { cid, .. } if cid == me);
                if mine && admission.live_at(now_ms) {
                    out.push(SharedItem {
                        volume: tree.volume.clone(),
                        entry: entry.id,
                        by: admission.by.clone(),
                        expires_ms: admission.expires_ms,
                    });
                }
            }
        }
    }
    out
}

/// The entries an outsider is served for an admission on `root`: `root` and what lies under
/// it, removed ones (and what lies under them) left out. `None` if `root` holds no live
/// admission for `actor`.
pub fn outsider_view(
    tree: &Tree,
    actor: &Principal,
    root: InodeId,
    now_ms: u64,
) -> Option<Vec<Entry>> {
    let admitted = tree.admissions_on(&root).any(|(_, a)| {
        a.live_at(now_ms)
            && match (&a.who, actor) {
                (Admitted::Member { cid, .. }, Principal::Member { cid: who }) => cid == who,
                (
                    Admitted::Link { link_id, .. },
                    Principal::LinkGuest {
                        link_id: own,
                        key_fp,
                    },
                ) => {
                    link_id == own
                        && a.claims.iter().any(|k| k == key_fp)
                        && !a.handed_over.contains(key_fp)
                }
                _ => false,
            }
    });
    if !admitted || tree.is_removed(&root) {
        return None;
    }
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let Some(entry) = tree.entry(&id) else {
            continue;
        };
        if entry.removal().is_some() {
            continue;
        }
        let mut entry = entry.clone();
        if id != root {
            entry.admissions.clear();
        } else {
            entry.admissions.retain(|_, a| {
                a.value.as_ref().is_some_and(|a| match (&a.who, actor) {
                    (Admitted::Member { cid, .. }, Principal::Member { cid: who }) => cid == who,
                    (Admitted::Link { link_id, .. }, Principal::LinkGuest { link_id: own, .. }) => {
                        link_id == own
                    }
                    _ => false,
                })
            });
        }
        out.push(entry);
        stack.extend(tree.children(&id).copied());
    }
    out.sort_by_key(|e| e.id);
    Some(out)
}

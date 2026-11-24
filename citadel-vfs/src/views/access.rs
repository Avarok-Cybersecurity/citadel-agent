//! What one actor may do to one entry, and who sees its name (`vfs-ui.md` §7).

use crate::authorize::{
    admitter_at, authorize, may_list, may_read_bytes, may_see, Denied, VolumeCtx,
};
use crate::hash::sha3_256;
use crate::inode::InodeId;
use crate::limits::DEFAULT_FILE_MODE;
use crate::model::{AdmittedKey, EntryKind, Mode, Pointee};
use crate::name::Name;
use crate::op::{op_hash, Body, Header, Op};
use crate::principal::Principal;
use crate::reduce::Tree;

/// Each answer is what the reader or `authorize` would say now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    pub see: Result<(), Denied>,
    pub list: Result<(), Denied>,
    pub read: Result<(), Denied>,
    pub write: Result<(), Denied>,
    pub create_in: Result<(), Denied>,
    pub remove: Result<(), Denied>,
    pub chmod: Result<(), Denied>,
}

fn probe(
    tree: &Tree,
    ctx: &VolumeCtx,
    actor: &Principal,
    target: InodeId,
    body: Body,
) -> Result<(), Denied> {
    let header = Header {
        volume: tree.volume.to_string(),
        op_id: [0; 16],
        author: actor.clone(),
        lamport: 1,
        prev: None,
        kind: body.kind(),
        target,
        body_hash: sha3_256(&[&body.encode()]),
        at_ms: ctx.now_ms,
    };
    let op = Op {
        op_hash: op_hash(&header.encode()),
        header,
        body: Some(body),
    };
    authorize(tree, ctx, &op)
}

/// A fresh id for a probe `Create`: one no entry holds.
fn unused_id(tree: &Tree, dir: &InodeId) -> InodeId {
    let mut id = InodeId([0; 16]);
    id.0.copy_from_slice(&sha3_256(&[b"citadel-vfs-probe", &dir.0])[..16]);
    while tree.exists(&id) || id == tree.root {
        id.0[15] = id.0[15].wrapping_add(1);
    }
    id
}

pub fn effective_access(tree: &Tree, ctx: &VolumeCtx, actor: &Principal, entry: InodeId) -> Access {
    let not_a = |what| Err(what);
    let current = tree.entry(&entry);
    let is_dir = entry == tree.root || current.is_some_and(|e| e.kind == EntryKind::Dir);
    let write = match current {
        Some(e) if e.kind == EntryKind::File => probe(
            tree,
            ctx,
            actor,
            entry,
            Body::SetPointee {
                pointee: e.pointee.value.clone(),
            },
        ),
        _ => not_a(Denied::NotAFile),
    };
    let create_in = if is_dir {
        let owner = admitter_at(tree, actor, &entry, ctx.now_ms).unwrap_or_else(|| actor.clone());
        match (Name::new("probe"), Mode::new(DEFAULT_FILE_MODE)) {
            (Ok(name), Some(mode)) => probe(
                tree,
                ctx,
                actor,
                unused_id(tree, &entry),
                Body::Create {
                    kind: EntryKind::File,
                    parent: entry,
                    name,
                    pointee: Pointee::Standard {
                        sha256: [0; 32],
                        len: 0,
                        mime: String::new(),
                    },
                    mode,
                    owner,
                },
            ),
            _ => not_a(Denied::NotAdmitted),
        }
    } else {
        not_a(Denied::NotAdmitted)
    };
    let chmod = match current {
        Some(e) => probe(tree, ctx, actor, entry, Body::Chmod { mode: e.mode.value }),
        None => not_a(Denied::NotAdmitted),
    };
    Access {
        see: may_see(tree, ctx, actor, entry),
        list: if is_dir {
            may_list(tree, ctx, actor, entry)
        } else {
            not_a(Denied::NotAdmitted)
        },
        read: may_read_bytes(tree, ctx, actor, entry),
        write,
        create_in,
        remove: probe(tree, ctx, actor, entry, Body::Remove),
        chmod,
    }
}

/// Who sees an entry's name: every member of its volume, and each outsider with a live
/// admission at it or above it while nothing between is removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamesSeenBy {
    pub every_member: bool,
    pub outsiders: Vec<(InodeId, AdmittedKey)>,
}

pub fn names_seen_by(tree: &Tree, entry: InodeId, now_ms: u64) -> NamesSeenBy {
    let mut chain = tree.ancestors(&entry);
    chain.push(entry);
    let mut outsiders = Vec::new();
    for (i, at) in chain.iter().enumerate() {
        let removed_between = chain[i..]
            .iter()
            .any(|a| tree.entry(a).is_some_and(|e| e.removal().is_some()));
        if removed_between {
            continue;
        }
        for (key, admission) in tree.admissions_on(at) {
            if admission.live_at(now_ms) {
                outsiders.push((*at, *key));
            }
        }
    }
    NamesSeenBy {
        every_member: true,
        outsiders,
    }
}

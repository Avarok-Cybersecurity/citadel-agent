//! RE-VFS trees (`vfs-migration.md` §3, §4).

use super::legacy::{Received, RevfsFileMetadata, RevfsFileState, RevfsNode, RevfsNodeType};
use super::names::node_name;
use super::plan::{emit, seed, tolerant_mime, Entry, Plan};
use super::{Migration, SkipReason};
use crate::inode::{InodeId, Namespace};
use crate::model::{EntryKind, Pointee};
use crate::principal::Principal;
use crate::volume::VolumeId;
use std::collections::BTreeMap;

const SENT: &str = "Sent Files";
const RECEIVED: &str = "Received Files";
const TRANSFERS: &str = "Transfers";

type FileRule<'a> =
    &'a dyn Fn(&RevfsFileMetadata, RevfsFileState) -> Result<(Pointee, Principal), SkipReason>;

struct Walk<'a> {
    volume: &'a VolumeId,
    me: u64,
    file: FileRule<'a>,
    out: Migration,
}

impl Walk<'_> {
    /// The plans for `nodes` under the directory `parent` whose `revfs-dir` key is `at`.
    fn dir(&mut self, nodes: &[RevfsNode], parent: InodeId, at: &str) -> Vec<Plan> {
        let mut plans = Vec::new();
        for node in nodes {
            let Some(name) = node_name(&node.name) else {
                self.out.skip(node.path.clone(), SkipReason::BadName);
                continue;
            };
            let planned = match node.node_type {
                RevfsNodeType::Directory => {
                    let key = format!("{at}/{}", name.as_str());
                    let entry = Entry {
                        volume: self.volume,
                        kind: EntryKind::Dir,
                        namespace: Namespace::RevfsDir,
                        key: key.clone(),
                        parent,
                        name,
                        pointee: Pointee::None,
                        owner: Principal::member(self.me),
                        at_ms: node.created_at,
                    };
                    entry.plan().map(|mut plan| {
                        plan.children = self.dir(&node.children, plan.draft.target, &key);
                        plan
                    })
                }
                RevfsNodeType::File => self.file(node, parent, name),
            };
            match planned {
                Ok(plan) => plans.push(plan),
                Err(why) => self.out.skip(node.path.clone(), why),
            }
        }
        plans
    }

    fn file(
        &self,
        node: &RevfsNode,
        parent: InodeId,
        name: crate::name::Name,
    ) -> Result<Plan, SkipReason> {
        let meta = node.file_metadata.as_ref().ok_or(SkipReason::NoMetadata)?;
        let state = node.file_state.ok_or(SkipReason::Inconsistent)?;
        let (pointee, owner) = (self.file)(meta, state)?;
        Entry {
            volume: self.volume,
            kind: EntryKind::File,
            namespace: Namespace::RevfsFile,
            key: meta.file_id.clone(),
            parent,
            name,
            pointee: tolerant_mime(pointee),
            owner,
            at_ms: node.created_at,
        }
        .plan()
    }
}

fn revfs(owner: u64, host: Principal, meta: &RevfsFileMetadata) -> Pointee {
    Pointee::Revfs {
        owner,
        host,
        virtual_path: meta.virtual_directory.clone(),
        len: meta.file_size,
        mime: meta.file_type.clone(),
    }
}

/// The per-peer tree `${me}_${peer}` into `pair:<lo>:<hi>`. `received` holds the received
/// transfers whose bytes are still on disk, by transfer id.
pub fn migrate_peer_tree(
    me: u64,
    peer: u64,
    tree: &RevfsNode,
    received: &BTreeMap<String, Received>,
) -> Migration {
    let Some(volume) = VolumeId::pair(me, peer) else {
        let mut out = Migration::default();
        out.skip(tree.path.clone(), SkipReason::Inconsistent);
        return out;
    };
    let rule = |meta: &RevfsFileMetadata, state| match state {
        RevfsFileState::Remote if meta.uploaded_by_cid == me => Ok((
            revfs(me, Principal::member(peer), meta),
            Principal::member(me),
        )),
        RevfsFileState::Hosted if meta.uploaded_by_cid == peer => Ok((
            revfs(peer, Principal::member(me), meta),
            Principal::member(peer),
        )),
        _ => Err(SkipReason::Inconsistent),
    };
    let mut walk = Walk {
        volume: &volume,
        me,
        file: &rule,
        out: Migration::default(),
    };
    let root = InodeId::root(&volume);
    let is_record = |n: &RevfsNode| {
        n.node_type == RevfsNodeType::Directory && (n.name == SENT || n.name == RECEIVED)
    };
    let ordinary: Vec<RevfsNode> = tree
        .children
        .iter()
        .filter(|n| !is_record(n))
        .cloned()
        .collect();
    let mut plans = walk.dir(&ordinary, root, "");
    let mut transfers: Vec<Plan> = Vec::new();
    for folder in tree.children.iter().filter(|n| is_record(n)) {
        for node in &folder.children {
            match record(&volume, me, folder.name == RECEIVED, node, received) {
                Ok(plan) => transfers.push(plan),
                Err(why) => walk.out.skip(node.path.clone(), why),
            }
        }
    }
    if let Some(first) = transfers.iter().map(|p| p.draft.at_ms).min() {
        match seed(&volume, TRANSFERS, Principal::member(me), first) {
            Ok(mut dir) => {
                for plan in &mut transfers {
                    if let crate::op::Body::Create { parent, .. } = &mut plan.draft.body {
                        *parent = dir.draft.target;
                    }
                }
                dir.children = transfers;
                plans.push(dir);
            }
            Err(why) => walk.out.skip(TRANSFERS, why),
        }
    }
    let mut out = walk.out;
    emit(plans, &mut out);
    out
}

/// A node under `/Sent Files` or `/Received Files`: a received transfer whose bytes are on
/// disk becomes a file under `/Transfers` (its parent set by the caller).
fn record(
    volume: &VolumeId,
    me: u64,
    in_received: bool,
    node: &RevfsNode,
    received: &BTreeMap<String, Received>,
) -> Result<Plan, SkipReason> {
    if node.node_type == RevfsNodeType::Directory {
        return Err(SkipReason::Inconsistent);
    }
    let meta = node.file_metadata.as_ref().ok_or(SkipReason::NoMetadata)?;
    match (in_received, node.file_state) {
        (false, Some(RevfsFileState::Sent)) => return Err(SkipReason::SentRecord),
        (true, Some(RevfsFileState::Received)) => {}
        _ => return Err(SkipReason::Inconsistent),
    }
    let name = node_name(&node.name).ok_or(SkipReason::BadName)?;
    let bytes = received.get(&meta.file_id).ok_or(SkipReason::BytesGone)?;
    Entry {
        volume,
        kind: EntryKind::File,
        namespace: Namespace::Transfer,
        key: meta.file_id.clone(),
        parent: InodeId::root(volume),
        name,
        pointee: tolerant_mime(Pointee::Standard {
            sha256: bytes.sha256,
            len: bytes.len,
            mime: meta.file_type.clone(),
        }),
        owner: Principal::member(me),
        at_ms: node.created_at,
    }
    .plan()
}

/// The server-scope tree `server_${cid}` into `personal:<me>`: every file is `ServerStored`.
pub fn migrate_server_tree(me: u64, tree: &RevfsNode) -> Migration {
    let volume = VolumeId::Personal { cid: me };
    let rule = |meta: &RevfsFileMetadata, state| match state {
        RevfsFileState::ServerStored => {
            Ok((revfs(me, Principal::Server, meta), Principal::member(me)))
        }
        _ => Err(SkipReason::Inconsistent),
    };
    let mut walk = Walk {
        volume: &volume,
        me,
        file: &rule,
        out: Migration::default(),
    };
    let plans = walk.dir(&tree.children, InodeId::root(&volume), "");
    let mut out = walk.out;
    emit(plans, &mut out);
    out
}

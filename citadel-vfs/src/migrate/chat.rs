//! Chat documents and files (`vfs-migration.md` §5, §6).

use super::legacy::{AttachmentEntry, IndexRecord, ShareSource, TitleSource};
use super::names::legacy_name;
use super::plan::{emit, seed, tolerant_mime, Entry, Plan};
use super::{Migration, SkipReason};
use crate::hash::from_hex;
use crate::inode::{InodeId, Namespace};
use crate::model::{EntryKind, Pointee};
use crate::principal::Principal;
use crate::volume::VolumeId;
use std::collections::BTreeMap;

const DOCUMENTS: &str = "Documents";
const CHAT_FILES: &str = "Chat files";

/// One document under the seeded `/Documents`, its parent set when the seed is planned.
fn document(
    volume: &VolumeId,
    doc_id: &str,
    title: Option<&str>,
    owner: Principal,
    at_ms: u64,
) -> Result<Plan, SkipReason> {
    let name = match title {
        Some(title) => legacy_name(title, doc_id),
        None => crate::name::Name::new(doc_id).ok(),
    }
    .ok_or(SkipReason::BadName)?;
    Entry {
        volume,
        kind: EntryKind::File,
        namespace: Namespace::ChatDoc,
        key: doc_id.to_string(),
        parent: InodeId::root(volume),
        name,
        pointee: Pointee::LiveDoc {
            doc_id: doc_id.to_string(),
        },
        owner,
        at_ms,
    }
    .plan()
}

/// The seeded directory `name`, owned by `owner`, holding `plans` (none: nothing is planned).
fn under_seed(
    volume: &VolumeId,
    name: &str,
    owner: Principal,
    mut plans: Vec<Plan>,
    out: &mut Migration,
) {
    let Some(first) = plans.iter().map(|p| p.draft.at_ms).min() else {
        return;
    };
    match seed(volume, name, owner, first) {
        Ok(mut dir) => {
            for plan in &mut plans {
                if let crate::op::Body::Create { parent, .. } = &mut plan.draft.body {
                    *parent = dir.draft.target;
                }
            }
            dir.children = plans;
            emit(vec![dir], out);
        }
        Err(why) => out.skip(name, why),
    }
}

/// The direct-chat and scratchpad index into `/Documents` of each volume it names.
pub fn migrate_chat_index(me: u64, entries: &[IndexRecord]) -> BTreeMap<VolumeId, Migration> {
    let mut per: BTreeMap<VolumeId, (Vec<Plan>, Migration)> = BTreeMap::new();
    let mut orphans = Migration::default();
    for record in entries {
        let volume = if record.peer_cid == me {
            VolumeId::Device { cid: me }
        } else {
            match VolumeId::pair(me, record.peer_cid) {
                Some(volume) => volume,
                None => {
                    orphans.skip(record.doc_id.clone(), SkipReason::Inconsistent);
                    continue;
                }
            }
        };
        let title = record.title.as_ref();
        let at_ms = title.map_or(0, |t| t.at);
        let (plans, out) = per.entry(volume.clone()).or_default();
        match document(
            &volume,
            &record.doc_id,
            title.map(|t| t.title.as_str()),
            Principal::member(me),
            at_ms,
        ) {
            Ok(plan) => plans.push(plan),
            Err(why) => out.skip(record.doc_id.clone(), why),
        }
    }
    let mut result = BTreeMap::new();
    for (volume, (plans, mut out)) in per {
        under_seed(&volume, DOCUMENTS, Principal::member(me), plans, &mut out);
        result.insert(volume, out);
    }
    if !orphans.skipped.is_empty() {
        result
            .entry(VolumeId::Device { cid: me })
            .or_insert_with(Migration::default)
            .skipped
            .extend(orphans.skipped);
    }
    result
}

/// A node's chat documents and shared files into `node:<node_id>`, authored by `Server`.
pub fn migrate_node_chat(
    node_id: &str,
    documents: &[(String, Option<TitleSource>)],
    files: &[(String, AttachmentEntry, Option<ShareSource>)],
) -> Migration {
    let mut out = Migration::default();
    let Ok(volume) = VolumeId::node(node_id) else {
        out.skip(node_id, SkipReason::Inconsistent);
        return out;
    };
    let volume = &volume;
    let mut docs = Vec::new();
    for (doc_id, source) in documents {
        let (title, owner) = match source {
            Some(s) => (Some(s.content.as_str()), Principal::member(s.sender_cid)),
            None => (None, Principal::Server),
        };
        match document(volume, doc_id, title, owner, 0) {
            Ok(plan) => docs.push(plan),
            Err(why) => out.skip(doc_id.clone(), why),
        }
    }
    under_seed(volume, DOCUMENTS, Principal::Server, docs, &mut out);
    let mut shared = Vec::new();
    for (attachment_id, entry, share) in files {
        match attachment(volume, attachment_id, entry, share.as_ref()) {
            Ok(plan) => shared.push(plan),
            Err(why) => out.skip(attachment_id.clone(), why),
        }
    }
    under_seed(volume, CHAT_FILES, Principal::Server, shared, &mut out);
    out
}

fn attachment(
    volume: &VolumeId,
    id: &str,
    entry: &AttachmentEntry,
    share: Option<&ShareSource>,
) -> Result<Plan, SkipReason> {
    if entry.messages.is_empty() {
        return Err(SkipReason::UnsharedUpload);
    }
    let sha256 = from_hex::<32>(id).ok_or(SkipReason::Inconsistent)?;
    let name = match share {
        Some(s) => legacy_name(&s.name, id),
        None => crate::name::Name::new(id).ok(),
    }
    .ok_or(SkipReason::BadName)?;
    let owner = entry
        .uploaders
        .values()
        .next()
        .map_or(Principal::Server, |cid| Principal::member(*cid));
    let mime = share.map(|s| s.mime.clone()).unwrap_or_default();
    Entry {
        volume,
        kind: EntryKind::File,
        namespace: Namespace::ChatFile,
        key: id.to_string(),
        parent: InodeId::root(volume),
        name,
        pointee: tolerant_mime(Pointee::Standard {
            sha256,
            len: entry.size_bytes,
            mime,
        }),
        owner,
        at_ms: entry.uploaded_at_ms,
    }
    .plan()
}

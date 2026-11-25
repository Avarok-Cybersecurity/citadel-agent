//! Legacy shapes as the page stored them (`tree.json`, camelCase), and drafts described in one
//! line each so a corpus file can list what a pass must write.

use super::J;
use citadel_vfs::migrate::{OpDraft, RevfsFileMetadata, RevfsFileState, RevfsNode, RevfsNodeType};
use citadel_vfs::model::Pointee;
use citadel_vfs::op::{Body, Draft, Op};
use citadel_vfs::{InodeId, Namespace, Principal, VolumeId};

pub fn node(j: &J) -> RevfsNode {
    let text = |k: &str| {
        j[k].as_str()
            .unwrap_or_else(|| panic!("{k} in {j}"))
            .to_string()
    };
    RevfsNode {
        name: text("name"),
        node_type: if j["type"] == "directory" {
            RevfsNodeType::Directory
        } else {
            RevfsNodeType::File
        },
        path: text("path"),
        children: j["children"]
            .as_array()
            .into_iter()
            .flatten()
            .map(node)
            .collect(),
        file_state: j["fileState"].as_str().map(|s| match s {
            "Hosted" => RevfsFileState::Hosted,
            "Remote" => RevfsFileState::Remote,
            "Sent" => RevfsFileState::Sent,
            "Received" => RevfsFileState::Received,
            "ServerStored" => RevfsFileState::ServerStored,
            other => panic!("state {other}"),
        }),
        file_metadata: j
            .get("fileMetadata")
            .filter(|m| !m.is_null())
            .map(|m| RevfsFileMetadata {
                file_id: m["fileId"].as_str().unwrap_or_default().to_string(),
                file_name: m["fileName"].as_str().unwrap_or_default().to_string(),
                file_size: m["fileSize"].as_u64().unwrap_or_default(),
                file_type: m["fileType"].as_str().unwrap_or_default().to_string(),
                virtual_directory: m["virtualDirectory"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                uploaded_by_cid: m["uploadedByCid"].as_u64().unwrap_or_default(),
            }),
        created_at: j["createdAt"].as_u64().unwrap_or_default(),
    }
}

fn principal(p: &Principal) -> String {
    match p {
        Principal::Server => "server".into(),
        Principal::Member { cid } => cid.to_string(),
        Principal::LinkGuest { key_fp, .. } => format!("guest:{key_fp}"),
    }
}

fn pointee(p: &Pointee) -> String {
    match p {
        Pointee::None => "none".into(),
        Pointee::Standard { sha256, len, mime } => format!(
            "standard({}…,{len},{mime})",
            &citadel_vfs::to_hex(sha256)[..8]
        ),
        Pointee::Revfs {
            owner,
            host,
            virtual_path,
            len,
            mime,
        } => {
            format!(
                "revfs(owner {owner},host {},{virtual_path},{len},{mime})",
                principal(host)
            )
        }
        Pointee::LiveDoc { doc_id } => format!("doc({doc_id})"),
    }
}

/// `<ns>:<key> in <parent ns:key | root> "<name>" <pointee> owner <p> mode <o> at <ms>`.
pub fn describe(d: &OpDraft, volume: &VolumeId, parents: &[(InodeId, String)]) -> String {
    let Body::Create {
        parent,
        name,
        pointee: p,
        mode,
        owner,
        ..
    } = &d.body
    else {
        panic!("a migration writes only Create");
    };
    let parent = if *parent == InodeId::root(volume) {
        "root".to_string()
    } else {
        parents
            .iter()
            .find(|(id, _)| id == parent)
            .map(|(_, k)| k.clone())
            .unwrap_or_else(|| "?".into())
    };
    let ns = Namespace::parse(d.namespace).unwrap_or_else(|| panic!("namespace {}", d.namespace));
    assert_eq!(
        InodeId::derive(volume, ns, &d.key).ok(),
        Some(d.target),
        "target is derived"
    );
    format!(
        "{}:{} in {parent} \"{}\" {} owner {} mode {:o} at {}",
        d.namespace,
        d.key,
        name.as_str(),
        pointee(p),
        principal(owner),
        mode.bits(),
        d.at_ms
    )
}

pub fn describe_all(ops: &[OpDraft], volume: &VolumeId) -> Vec<String> {
    let mut parents = Vec::new();
    ops.iter()
        .map(|d| {
            let line = describe(d, volume, &parents);
            parents.push((d.target, format!("{}:{}", d.namespace, d.key)));
            line
        })
        .collect()
}

/// The drafts stamped as `author` would stamp them: lamport from 1, each `prev` the last.
pub fn stamp(ops: &[OpDraft], volume: &VolumeId, author: &Principal) -> Vec<Op> {
    let mut prev = None;
    ops.iter()
        .enumerate()
        .map(|(i, d)| {
            let op = Draft {
                volume: volume.clone(),
                op_id: d.op_id(volume, author).unwrap_or_else(|_| panic!("op_id")),
                author: author.clone(),
                lamport: i as u64 + 1,
                prev,
                target: d.target,
                at_ms: d.at_ms,
                body: d.body.clone(),
            }
            .to_op();
            prev = Some(op.op_hash);
            op
        })
        .collect()
}

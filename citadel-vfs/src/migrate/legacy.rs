//! The legacy shapes, as the code that wrote them has them (`vfs-migration.md` §1).

use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevfsNodeType {
    File,
    Directory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevfsFileState {
    Hosted,
    Remote,
    Sent,
    Received,
    ServerStored,
}

/// `RevfsFileMetadata` (`citadel-workspaces/src/types/revfs-types.ts`); `thumbnail` is not moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevfsFileMetadata {
    pub file_id: String,
    pub file_name: String,
    pub file_size: u64,
    pub file_type: String,
    pub virtual_directory: String,
    pub uploaded_by_cid: u64,
}

/// `RevfsNode`; `updatedAt` is not moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevfsNode {
    pub name: String,
    pub node_type: RevfsNodeType,
    pub path: String,
    pub children: Vec<RevfsNode>,
    pub file_state: Option<RevfsFileState>,
    pub file_metadata: Option<RevfsFileMetadata>,
    pub created_at: u64,
}

/// A received transfer's bytes still on disk, hashed by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Received {
    pub sha256: [u8; 32],
    pub len: u64,
}

/// `CdocChatTitleStamp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleStamp {
    pub title: String,
    pub at: u64,
    pub by: u64,
}

/// `IndexRecord` of `cdoc-chat:<account>:#index`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRecord {
    pub doc_id: String,
    pub peer_cid: u64,
    pub title: Option<TitleStamp>,
}

/// The node-chat message of type `LiveDocument { document_id }` that names a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleSource {
    pub content: String,
    pub sender_cid: u64,
}

/// `AttachmentEntry`; `uploaders` maps each uploader's username to the cid the kernel resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentEntry {
    pub size_bytes: u64,
    pub uploaders: BTreeMap<String, u64>,
    pub messages: BTreeSet<u64>,
    pub uploaded_at_ms: u64,
}

/// The `FileShare { attachment_id, name, size_bytes, mime }` message with the smallest id in
/// the entry's `messages`, chosen by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareSource {
    pub name: String,
    pub mime: String,
}

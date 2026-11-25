//! Migration (`vfs-migration.md`): pure functions from each legacy shape to the ops that recreate
//! it. The caller stamps, signs and appends each draft; ids are derived, so a pass run twice, or
//! by both members of a pair, gives one tree.

mod chat;
mod legacy;
mod names;
mod plan;
mod revfs;

pub use chat::{migrate_chat_index, migrate_node_chat};
pub use legacy::{
    AttachmentEntry, IndexRecord, Received, RevfsFileMetadata, RevfsFileState, RevfsNode,
    RevfsNodeType, ShareSource, TitleSource, TitleStamp,
};
pub use names::legacy_name;
pub use revfs::{migrate_peer_tree, migrate_server_tree};

use crate::hash::{lp, sha3_256, TooLong};
use crate::inode::InodeId;
use crate::op::{Body, OpKind};
use crate::principal::Principal;
use crate::volume::VolumeId;

pub const MIGRATE_DOMAIN: &[u8] = b"citadel-vfs-migrate-v1";

/// One op to append, before its stamp (`lamport`, `prev`) and signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpDraft {
    pub kind: OpKind,
    pub target: InodeId,
    pub body: Body,
    pub at_ms: u64,
    pub namespace: &'static str,
    pub key: String,
}

impl OpDraft {
    /// The draft's `op_id` for `author` in `volume` (`vfs-ops.md` §2).
    pub fn op_id(&self, volume: &VolumeId, author: &Principal) -> Result<[u8; 16], TooLong> {
        migration_op_id(volume, author, self.namespace, &self.key, self.kind)
    }
}

pub fn migration_op_id(
    volume: &VolumeId,
    author: &Principal,
    namespace: &str,
    key: &str,
    kind: OpKind,
) -> Result<[u8; 16], TooLong> {
    let mut framed = Vec::new();
    lp(&mut framed, &volume.to_string())?;
    framed.extend_from_slice(&author.encode());
    lp(&mut framed, namespace)?;
    lp(&mut framed, key)?;
    framed.push(u8::try_from(kind.code()).map_err(|_| TooLong)?);
    let digest = sha3_256(&[MIGRATE_DOMAIN, &[0], &framed]);
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[..16]);
    Ok(id)
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum SkipReason {
    BadName,
    NoMetadata,
    Inconsistent,
    BytesGone,
    SentRecord,
    UnsharedUpload,
    /// A file this member stores for the peer: it appears when the peer's own pass runs.
    StoredForPeer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedLegacy {
    pub what: String,
    pub why: SkipReason,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Migration {
    pub ops: Vec<OpDraft>,
    pub skipped: Vec<SkippedLegacy>,
}

impl Migration {
    pub(crate) fn skip(&mut self, what: impl Into<String>, why: SkipReason) {
        self.skipped.push(SkippedLegacy {
            what: what.into(),
            why,
        });
    }
}

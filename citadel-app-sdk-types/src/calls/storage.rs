//! `storage`: the kernel's named backends and the host-local KV (R4.5). Keys are namespaced by
//! app id and scope from the install record, never from these arguments.
//!
//! `storage.node.authored` is one KV per node per author: anyone who may view the node writes to
//! their own, and reads anyone's. The author a write lands under is the session's, never an
//! argument; `author` on a write may only repeat it.

use crate::{Bytes, Capability};
use alloc::string::String;
use alloc::vec::Vec;

abi_types! {
    /// Which KV a `Kv*` op addresses; it decides the capability the op needs.
    #[serde(rename_all = "snake_case")]
    pub enum KvBackend {
        User,
        Scope,
        Node { node_id: String },
        Local,
        /// `author`: whose namespace a read addresses, None the caller's own. A write always
        /// lands in the caller's own; naming anyone else is refused.
        NodeAuthored { node_id: String, author: Option<String> },
    }

    /// Which store changed (`AppEvent::StorageChanged`).
    #[derive(Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum StorageBackend {
        User,
        Scope,
        Node { node_id: String },
        Local,
        Blob,
        Files,
        NodeAuthored { node_id: String, author: String },
    }

    pub struct KvGet {
        pub backend: KvBackend,
        pub key: String,
    }

    pub struct KvValue {
        pub value: Option<Bytes>,
    }

    pub struct KvSet {
        pub backend: KvBackend,
        pub key: String,
        pub value: Bytes,
    }

    pub struct KvEntry {
        pub key: String,
        pub value: Bytes,
    }

    /// All or nothing.
    pub struct KvSetMany {
        pub backend: KvBackend,
        pub entries: Vec<KvEntry>,
    }

    pub struct KvDelete {
        pub backend: KvBackend,
        pub key: String,
    }

    /// One page of keys under `prefix`; pass `next` back as `cursor`.
    pub struct KvList {
        pub backend: KvBackend,
        pub prefix: String,
        pub cursor: Option<String>,
    }

    pub struct KvPage {
        pub keys: Vec<String>,
        pub next: Option<String>,
    }

    /// One page of every author's keys under `prefix` on `node_id` (`storage.node.authored`),
    /// by author and then key; pass `next` back as `cursor`.
    pub struct KvListAuthored {
        pub node_id: String,
        pub prefix: String,
        pub cursor: Option<String>,
    }

    pub struct AuthoredKey {
        pub author: String,
        pub key: String,
    }

    pub struct AuthoredPage {
        pub entries: Vec<AuthoredKey>,
        pub next: Option<String>,
    }

    pub struct BlobPut {
        pub bytes: Bytes,
    }

    /// Lowercase hex sha256 of the blob.
    pub struct BlobRef {
        pub sha256: String,
    }

    /// Reply `BytesValue`.
    pub struct BlobGet {
        pub sha256: String,
    }

    pub struct BlobDelete {
        pub sha256: String,
    }

    /// Under the server file store's `/apps/<app>/<scope>/`.
    pub struct StoreFileWrite {
        pub path: String,
        pub bytes: Bytes,
    }

    /// Reply `BytesValue`.
    pub struct StoreFileRead {
        pub path: String,
    }

    /// Reply `FileList`.
    pub struct StoreFileList {
        pub prefix: String,
    }
}

impl KvBackend {
    pub const fn capability(&self) -> Capability {
        match self {
            KvBackend::User => Capability::StorageUser,
            KvBackend::Scope => Capability::StorageScope,
            KvBackend::Node { .. } => Capability::StorageNode,
            KvBackend::Local => Capability::StorageLocal,
            KvBackend::NodeAuthored { .. } => Capability::StorageNodeAuthored,
        }
    }
}

macro_rules! backend_decides {
    ($($op:ident),*) => {$(
        impl $op {
            /// The backend decides the capability: `storage.user`, `.scope`, `.node`, `.local`,
            /// `.node.authored`.
            pub const fn required_capability(&self) -> Capability {
                self.backend.capability()
            }
        }
    )*};
}

backend_decides!(KvGet, KvSet, KvSetMany, KvDelete, KvList);

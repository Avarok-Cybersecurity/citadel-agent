//! `storage`: the kernel's named backends and the host-local KV (R4.5). Keys are namespaced by
//! app id and scope from the install record, never from these arguments.

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
    }

    /// Which store changed (`AppEvent::StorageChanged`).
    #[serde(rename_all = "snake_case")]
    pub enum StorageBackend {
        User,
        Scope,
        Node { node_id: String },
        Local,
        Blob,
        Files,
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
        }
    }
}

impl From<KvBackend> for StorageBackend {
    fn from(b: KvBackend) -> Self {
        match b {
            KvBackend::User => StorageBackend::User,
            KvBackend::Scope => StorageBackend::Scope,
            KvBackend::Node { node_id } => StorageBackend::Node { node_id },
            KvBackend::Local => StorageBackend::Local,
        }
    }
}

macro_rules! backend_decides {
    ($($op:ident),*) => {$(
        impl $op {
            /// The backend decides the capability: `storage.user`, `.scope`, `.node`, `.local`.
            pub const fn required_capability(&self) -> Capability {
                self.backend.capability()
            }
        }
    )*};
}

backend_decides!(KvGet, KvSet, KvSetMany, KvDelete, KvList);

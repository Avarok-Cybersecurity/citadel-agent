//! `storage`: the named backends (R4.5). The host namespaces every key by app and scope; an app
//! can address only its own data.

use crate::{call, Pending};
use citadel_app_sdk_types::calls::*;
use citadel_app_sdk_types::Bytes;

/// One KV backend.
#[derive(Clone, Debug, PartialEq)]
pub struct Kv {
    backend: KvBackend,
}

/// Per user, per app and scope (`storage.user`).
pub fn user() -> Kv {
    Kv {
        backend: KvBackend::User,
    }
}

/// Shared by everyone the install covers (`storage.scope`).
pub fn scope() -> Kv {
    Kv {
        backend: KvBackend::Scope,
    }
}

/// Per node (`storage.node`).
pub fn node(node_id: &str) -> Kv {
    Kv {
        backend: KvBackend::Node {
            node_id: node_id.into(),
        },
    }
}

/// The caller's own namespace on a node (`storage.node.authored`): anyone who may view the node
/// writes here, and only here.
pub fn node_authored(node_id: &str) -> Kv {
    Kv {
        backend: KvBackend::NodeAuthored {
            node_id: node_id.into(),
            author: None,
        },
    }
}

/// Another member's namespace on a node (`storage.node.authored`), to read; a write is refused.
pub fn node_authored_by(node_id: &str, author: &str) -> AuthoredReader {
    AuthoredReader {
        backend: KvBackend::NodeAuthored {
            node_id: node_id.into(),
            author: Some(author.into()),
        },
    }
}

/// One page of every author's keys under `prefix` on a node, by author then key.
pub fn list_authored(node_id: &str, prefix: &str, cursor: Option<String>) -> Pending<AuthoredPage> {
    call(&KvListAuthored {
        node_id: node_id.into(),
        prefix: prefix.into(),
        cursor,
    })
}

/// Reads of one author's namespace on a node; it has no write, as the kernel admits none.
#[derive(Clone, Debug, PartialEq)]
pub struct AuthoredReader {
    backend: KvBackend,
}

impl AuthoredReader {
    pub fn get(&self, key: &str) -> Pending<Option<Vec<u8>>> {
        call(&KvGet {
            backend: self.backend.clone(),
            key: key.into(),
        })
        .map(|v| v.value.map(|b| b.0))
    }

    pub fn list(&self, prefix: &str, cursor: Option<String>) -> Pending<KvPage> {
        call(&KvList {
            backend: self.backend.clone(),
            prefix: prefix.into(),
            cursor,
        })
    }
}

/// On the member's own machine (`storage.local`).
pub fn local() -> Kv {
    Kv {
        backend: KvBackend::Local,
    }
}

impl Kv {
    pub fn get(&self, key: &str) -> Pending<Option<Vec<u8>>> {
        call(&KvGet {
            backend: self.backend.clone(),
            key: key.into(),
        })
        .map(|v| v.value.map(|b| b.0))
    }

    pub fn set(&self, key: &str, value: &[u8]) -> Pending<()> {
        call(&KvSet {
            backend: self.backend.clone(),
            key: key.into(),
            value: Bytes(value.to_vec()),
        })
    }

    /// All or nothing.
    pub fn set_many(&self, entries: &[(&str, &[u8])]) -> Pending<()> {
        let entries = entries
            .iter()
            .map(|(k, v)| KvEntry {
                key: (*k).into(),
                value: Bytes(v.to_vec()),
            })
            .collect();
        call(&KvSetMany {
            backend: self.backend.clone(),
            entries,
        })
    }

    pub fn delete(&self, key: &str) -> Pending<()> {
        call(&KvDelete {
            backend: self.backend.clone(),
            key: key.into(),
        })
    }

    /// One page; pass the page's `next` as `cursor` for the following one.
    pub fn list(&self, prefix: &str, cursor: Option<String>) -> Pending<KvPage> {
        call(&KvList {
            backend: self.backend.clone(),
            prefix: prefix.into(),
            cursor,
        })
    }
}

/// Content-addressed blobs (`storage.blob`).
pub mod blob {
    use super::*;

    /// The blob's sha256, lowercase hex.
    pub fn put(bytes: &[u8]) -> Pending<String> {
        call(&BlobPut {
            bytes: Bytes(bytes.to_vec()),
        })
        .map(|r| r.sha256)
    }

    pub fn get(sha256: &str) -> Pending<Vec<u8>> {
        call(&BlobGet {
            sha256: sha256.into(),
        })
        .map(|b| b.bytes.0)
    }

    pub fn delete(sha256: &str) -> Pending<()> {
        call(&BlobDelete {
            sha256: sha256.into(),
        })
    }
}

/// The app's directory in the server file store (`storage.files`).
pub mod files {
    use super::*;

    pub fn write(path: &str, bytes: &[u8]) -> Pending<()> {
        call(&StoreFileWrite {
            path: path.into(),
            bytes: Bytes(bytes.to_vec()),
        })
    }

    pub fn read(path: &str) -> Pending<Vec<u8>> {
        call(&StoreFileRead { path: path.into() }).map(|b| b.bytes.0)
    }

    pub fn list(prefix: &str) -> Pending<Vec<FileEntry>> {
        call(&StoreFileList {
            prefix: prefix.into(),
        })
        .map(|l| l.entries)
    }
}

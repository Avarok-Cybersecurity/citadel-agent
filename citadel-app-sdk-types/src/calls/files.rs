//! `files`: the workspace's server file store, under the install's scope.

use crate::Bytes;
use alloc::string::String;
use alloc::vec::Vec;

abi_types! {
    pub struct FileEntry {
        pub path: String,
        pub is_dir: bool,
        pub size: u64,
    }

    pub struct FileList {
        pub entries: Vec<FileEntry>,
    }

    pub struct FilesList {
        pub path: String,
    }

    /// `len` bytes from `offset`; reply `BytesValue`. The host caps `len`.
    pub struct FilesRead {
        pub path: String,
        pub offset: u64,
        pub len: u32,
    }

    pub struct FilesWrite {
        pub path: String,
        pub bytes: Bytes,
    }

    pub struct FilesDelete {
        pub path: String,
    }
}

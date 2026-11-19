//! Where the agent keeps its identity record (the sealed seed and the public key). The identity
//! is the agent's, not an account's, so it cannot live in an account's byte map: it exists
//! before the first account and outlives every deregistration.
//!
//! The binary chooses the adapter, as it chooses the SDK backend: a directory under the data dir
//! for the filesystem backend, memory otherwise. There is no default.

use async_trait::async_trait;
use parking_lot::Mutex;
use std::path::PathBuf;

#[async_trait]
pub trait IdentityStore: Send + Sync + 'static {
    /// The stored record, `None` when this agent holds no identity.
    async fn load(&self) -> Result<Option<Vec<u8>>, String>;
    /// Replaces the stored record.
    async fn save(&self, record: Vec<u8>) -> Result<(), String>;
}

/// A record held for the process's lifetime, for an agent whose accounts are in memory too.
#[derive(Default)]
pub struct MemoryIdentityStore(Mutex<Option<Vec<u8>>>);

#[async_trait]
impl IdentityStore for MemoryIdentityStore {
    async fn load(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(self.0.lock().clone())
    }

    async fn save(&self, record: Vec<u8>) -> Result<(), String> {
        *self.0.lock() = Some(record);
        Ok(())
    }
}

const FILE_NAME: &str = "identity.json";
const TEMP_NAME: &str = "identity.json.tmp";

/// `<dir>/identity.json`, written whole to a temporary file and renamed over the old one, so a
/// crash leaves either record and never half of one. The directory is created private (0700)
/// and the file 0600: the seed is sealed, but its Argon2id salt and the key are this
/// person's alone.
pub struct FsIdentityStore {
    dir: PathBuf,
}

impl FsIdentityStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
}

#[async_trait]
impl IdentityStore for FsIdentityStore {
    async fn load(&self) -> Result<Option<Vec<u8>>, String> {
        crate::kernel::private_file::read(&self.dir, FILE_NAME).await
    }

    async fn save(&self, record: Vec<u8>) -> Result<(), String> {
        crate::kernel::private_file::write(&self.dir, FILE_NAME, TEMP_NAME, record).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_file_store_round_trips_and_replaces_whole() {
        let dir = std::env::temp_dir().join(format!("identity-store-{}", uuid::Uuid::new_v4()));
        let store = FsIdentityStore::new(dir.clone());
        assert_eq!(store.load().await.unwrap(), None);
        store.save(b"first".to_vec()).await.unwrap();
        store.save(b"second".to_vec()).await.unwrap();
        assert_eq!(store.load().await.unwrap(), Some(b"second".to_vec()));
        assert!(!dir.join(TEMP_NAME).exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(FILE_NAME))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o077,
                0,
                "the record is readable by others: {mode:o}"
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}

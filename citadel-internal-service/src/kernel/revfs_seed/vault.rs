//! The agent's vault key: the one key every account's RE-VFS roots are sealed under at rest. It is
//! drawn once, the first time an account needs it, and kept where the binary says: a private
//! file beside the filesystem backend's accounts (never inside them), or memory for an agent
//! whose accounts are in memory too. There is no default store.
//!
//! The key is the agent's, not an account's: it never leaves this device and is never logged.

use super::seal::VAULT_KEY_LEN;
use async_trait::async_trait;
use citadel_sdk::prelude::BackendType;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use zeroize::Zeroizing;

#[async_trait]
pub trait RevfsVaultStore: Send + Sync + 'static {
    /// The stored vault key, `None` before the first account needed one.
    async fn load(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String>;
    async fn save(&self, key: Zeroizing<Vec<u8>>) -> Result<(), String>;
}

/// A vault key held for the process's lifetime, for an agent whose accounts are in memory too.
#[derive(Default)]
pub struct MemoryRevfsVault(Mutex<Option<Zeroizing<Vec<u8>>>>);

#[async_trait]
impl RevfsVaultStore for MemoryRevfsVault {
    async fn load(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        Ok(self.0.lock().clone())
    }

    async fn save(&self, key: Zeroizing<Vec<u8>>) -> Result<(), String> {
        *self.0.lock() = Some(key);
        Ok(())
    }
}

const FILE_NAME: &str = "revfs-vault.key";
const TEMP_NAME: &str = "revfs-vault.key.tmp";

/// `<dir>/revfs-vault.key`, private (0700 directory, 0600 file), written whole.
pub struct FsRevfsVault {
    dir: PathBuf,
}

impl FsRevfsVault {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
}

#[async_trait]
impl RevfsVaultStore for FsRevfsVault {
    async fn load(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        crate::kernel::private_file::read(&self.dir, FILE_NAME)
            .await
            .map(|key| key.map(Zeroizing::new))
    }

    async fn save(&self, key: Zeroizing<Vec<u8>>) -> Result<(), String> {
        crate::kernel::private_file::write(&self.dir, FILE_NAME, TEMP_NAME, key.to_vec()).await
    }
}

/// Where a vault key is kept: beside a filesystem backend's accounts (`<data dir>/revfs-vault`,
/// never inside an account), in memory with an in-memory backend. It persists exactly when the
/// roots sealed under it do.
pub fn vault_for(backend: &BackendType) -> Result<Arc<dyn RevfsVaultStore>, String> {
    match backend {
        BackendType::InMemory => Ok(Arc::new(MemoryRevfsVault::default())),
        #[cfg(not(target_family = "wasm"))]
        BackendType::Filesystem(dir) => Ok(Arc::new(FsRevfsVault::new(
            std::path::Path::new(dir).join("revfs-vault"),
        ))),
        #[allow(unreachable_patterns)]
        _ => Err("a RE-VFS vault is kept only beside an in-memory or filesystem backend".into()),
    }
}

/// The vault key, loaded or drawn once per process. Serialised, so two first registrations
/// cannot each draw a key and seal roots under the one the other overwrites. Also who is using
/// each account's key (`usage.rs`), which lives exactly as long as the vault.
pub(crate) struct Vault {
    store: Arc<dyn RevfsVaultStore>,
    key: tokio::sync::Mutex<Option<Arc<Zeroizing<[u8; VAULT_KEY_LEN]>>>>,
    pub(crate) accounts: super::usage::Accounts,
}

impl Vault {
    pub(crate) fn new(store: Arc<dyn RevfsVaultStore>) -> Self {
        Self {
            store,
            key: tokio::sync::Mutex::new(None),
            accounts: Default::default(),
        }
    }

    pub(crate) async fn key(&self) -> Result<Arc<Zeroizing<[u8; VAULT_KEY_LEN]>>, String> {
        let mut held = self.key.lock().await;
        if let Some(key) = held.as_ref() {
            return Ok(key.clone());
        }
        let key = match self.store.load().await? {
            Some(stored) => {
                let mut key = Zeroizing::new([0u8; VAULT_KEY_LEN]);
                if stored.len() != VAULT_KEY_LEN {
                    return Err("the stored RE-VFS vault key is not 32 bytes".to_string());
                }
                key.copy_from_slice(&stored);
                key
            }
            None => {
                let mut key = Zeroizing::new([0u8; VAULT_KEY_LEN]);
                getrandom::fill(key.as_mut_slice())
                    .map_err(|err| format!("no OS randomness for the RE-VFS vault key: {err}"))?;
                self.store.save(Zeroizing::new(key.to_vec())).await?;
                key
            }
        };
        let key = Arc::new(key);
        *held = Some(key.clone());
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_key_is_drawn_once_and_then_reloaded() {
        let store: Arc<dyn RevfsVaultStore> = Arc::new(MemoryRevfsVault::default());
        let first = Vault::new(store.clone()).key().await.unwrap();
        let again = Vault::new(store).key().await.unwrap();
        assert_eq!(
            **first, **again,
            "a restarted agent must open the roots it sealed"
        );
    }

    #[tokio::test]
    async fn the_file_vault_is_private_and_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("revfs-vault-{}", uuid::Uuid::new_v4()));
        let first = Vault::new(Arc::new(FsRevfsVault::new(dir.clone())))
            .key()
            .await
            .unwrap();
        let again = Vault::new(Arc::new(FsRevfsVault::new(dir.clone())))
            .key()
            .await
            .unwrap();
        assert_eq!(**first, **again);
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
                "the vault key is readable by others: {mode:o}"
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_stored_key_of_the_wrong_length_is_refused() {
        let store = Arc::new(MemoryRevfsVault::default());
        store.save(Zeroizing::new(vec![1; 16])).await.unwrap();
        assert!(Vault::new(store).key().await.is_err());
    }
}

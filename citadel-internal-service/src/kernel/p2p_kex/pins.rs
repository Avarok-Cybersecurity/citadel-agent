//! The pin book, shared with the node and kept where the binary says: a private file beside a
//! filesystem backend's accounts, or memory beside an in-memory backend. There is no default.
//!
//! Reads answer from memory. A first-sight pin is saved before it enters the book, so no exchange
//! is reported verified on a pin a crash could lose; every other change is saved by a writer task
//! after it is made. One save runs at a time, each writing the book as it then stands.

use super::book::{NotAccepted, PinBook};
use async_trait::async_trait;
use citadel_sdk::prelude::BackendType;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch;

#[cfg(test)]
#[path = "pins_tests.rs"]
mod tests;

#[async_trait]
pub trait PeerKeyStore: Send + Sync + 'static {
    /// The saved book, `None` before any peer was pinned.
    async fn load(&self) -> Result<Option<Vec<u8>>, String>;
    async fn save(&self, book: Vec<u8>) -> Result<(), String>;
}

/// Pins held for the process's lifetime, for an agent whose accounts are in memory too.
#[derive(Default)]
pub struct MemoryPeerKeyStore(Mutex<Option<Vec<u8>>>);

#[async_trait]
impl PeerKeyStore for MemoryPeerKeyStore {
    async fn load(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(self.0.lock().clone())
    }

    async fn save(&self, book: Vec<u8>) -> Result<(), String> {
        *self.0.lock() = Some(book);
        Ok(())
    }
}

const FILE_NAME: &str = "peer-identity-keys.json";
const TEMP_NAME: &str = "peer-identity-keys.json.tmp";

/// `<dir>/peer-identity-keys.json`, private (0700 directory, 0600 file), written whole.
pub struct FsPeerKeyStore {
    dir: PathBuf,
}

impl FsPeerKeyStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
}

#[async_trait]
impl PeerKeyStore for FsPeerKeyStore {
    async fn load(&self) -> Result<Option<Vec<u8>>, String> {
        crate::kernel::private_file::read(&self.dir, FILE_NAME).await
    }

    async fn save(&self, book: Vec<u8>) -> Result<(), String> {
        crate::kernel::private_file::write(&self.dir, FILE_NAME, TEMP_NAME, book).await
    }
}

/// Where pins are kept: `<data dir>/peer-identity-keys` beside a filesystem backend's accounts,
/// memory with an in-memory backend. They persist exactly when the accounts do.
pub fn peer_key_store_for(backend: &BackendType) -> Result<Arc<dyn PeerKeyStore>, String> {
    match backend {
        BackendType::InMemory => Ok(Arc::new(MemoryPeerKeyStore::default())),
        #[cfg(not(target_family = "wasm"))]
        BackendType::Filesystem(dir) => Ok(Arc::new(FsPeerKeyStore::new(
            std::path::Path::new(dir).join("peer-identity-keys"),
        ))),
        #[allow(unreachable_patterns)]
        _ => {
            Err("peer identity keys are kept only beside an in-memory or filesystem backend".into())
        }
    }
}

/// The book in memory, and a writer that saves it after each change.
pub(crate) struct PeerKeyPins {
    book: Mutex<PinBook>,
    changed: watch::Sender<u64>,
    store: Arc<dyn PeerKeyStore>,
    saving: tokio::sync::Mutex<()>,
}

impl PeerKeyPins {
    /// Loads the saved book and starts its writer. A saved book that does not parse is an
    /// error: pins are never silently dropped.
    pub(crate) async fn load(store: Arc<dyn PeerKeyStore>) -> Result<Arc<Self>, String> {
        let book = match store.load().await? {
            Some(bytes) => PinBook::decode(&bytes)?,
            None => PinBook::default(),
        };
        let (changed, mut seen) = watch::channel(0);
        let pins = Arc::new(Self {
            book: Mutex::new(book),
            changed,
            store,
            saving: tokio::sync::Mutex::new(()),
        });
        let weak = Arc::downgrade(&pins);
        tokio::spawn(async move {
            while seen.changed().await.is_ok() {
                let Some(pins) = weak.upgrade() else { break };
                let _one_save = pins.saving.lock().await;
                let bytes = pins.book.lock().encode();
                if let Err(err) = pins.store.save(bytes).await {
                    citadel_sdk::logging::error!(target: "citadel", "[P2pKex] the peer identity keys were not saved: {err}");
                }
            }
        });
        Ok(pins)
    }

    fn saved(&self) {
        self.changed.send_modify(|version| *version += 1);
    }

    pub(crate) fn pinned(&self, local: u64, peer: u64) -> Option<Vec<u8>> {
        self.book.lock().pin(local, peer).map(|pin| pin.key.clone())
    }

    /// The pinned key and the waiting one, for the person to see.
    pub(crate) fn view(&self, local: u64, peer: u64) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
        match self.book.lock().pin(local, peer) {
            Some(pin) => (Some(pin.key.clone()), pin.pending.clone()),
            None => (None, None),
        }
    }

    /// Pins `key` for a peer with no pin, returning once the book holding it is saved; only then
    /// is it in the book. `Err` leaves the peer unpinned.
    pub(crate) async fn pin_first(&self, local: u64, peer: u64, key: &[u8]) -> Result<(), String> {
        let _one_save = self.saving.lock().await;
        let bytes = {
            let mut next = self.book.lock().clone();
            if !next.pin_first(local, peer, key) {
                return Ok(());
            }
            next.encode()
        };
        self.store.save(bytes).await?;
        self.book.lock().pin_first(local, peer, key);
        Ok(())
    }

    /// `true` if the presented key is newly waiting (the person has not been told of it).
    pub(crate) fn record_change(&self, local: u64, peer: u64, presented: &[u8]) -> bool {
        let newly = self.book.lock().record_change(local, peer, presented);
        if newly {
            self.saved();
        }
        newly
    }

    pub(crate) fn accept(&self, local: u64, peer: u64, fp: &str) -> Result<Vec<u8>, NotAccepted> {
        let accepted = self.book.lock().accept(local, peer, fp)?;
        self.saved();
        Ok(accepted)
    }
}

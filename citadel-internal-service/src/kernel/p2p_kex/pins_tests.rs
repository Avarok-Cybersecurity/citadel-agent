//! A first-sight pin is kept before anything reads it, and a pin that could not be kept is not
//! used. The store is in memory; what it is handed is what a file would hold.
//!
//! Control: in `pin_first`, put the pin in the book before `store.save` → the first test reads it
//! while the save is held open and fails.

use super::*;
use tokio::sync::{oneshot, Mutex as AsyncMutex};

const ALICE: u64 = 10;
const BOB: u64 = 20;
const KEY: &[u8] = b"bob's identity key";

/// Holds the first save open until `release` fires; records what each save was handed.
struct HeldStore {
    release: AsyncMutex<Option<oneshot::Receiver<()>>>,
    saved: Mutex<Vec<Vec<u8>>>,
}

#[async_trait]
impl PeerKeyStore for HeldStore {
    async fn load(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }

    async fn save(&self, book: Vec<u8>) -> Result<(), String> {
        let held = self.release.lock().await.take();
        if let Some(release) = held {
            release.await.map_err(|_| "released".to_string())?;
        }
        self.saved.lock().push(book);
        Ok(())
    }
}

struct FullDisk;

#[async_trait]
impl PeerKeyStore for FullDisk {
    async fn load(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }

    async fn save(&self, _book: Vec<u8>) -> Result<(), String> {
        Err("the disk is full".to_string())
    }
}

#[tokio::test]
async fn a_first_pin_is_saved_before_it_is_read_as_pinned() {
    let (release, held) = oneshot::channel();
    let store = Arc::new(HeldStore {
        release: AsyncMutex::new(Some(held)),
        saved: Mutex::default(),
    });
    let pins = PeerKeyPins::load(store.clone()).await.unwrap();
    let pinning = tokio::spawn({
        let pins = pins.clone();
        async move { pins.pin_first(BOB, ALICE, KEY).await }
    });
    while store.release.lock().await.is_some() {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        pins.pinned(BOB, ALICE),
        None,
        "not pinned while its save is open"
    );
    release.send(()).unwrap();
    pinning.await.unwrap().unwrap();
    assert_eq!(pins.pinned(BOB, ALICE), Some(KEY.to_vec()));
    let kept = PinBook::decode(store.saved.lock().last().unwrap()).unwrap();
    assert_eq!(
        kept.pin(BOB, ALICE).map(|pin| pin.key.clone()),
        Some(KEY.to_vec())
    );
}

#[tokio::test]
async fn a_pin_that_cannot_be_saved_is_not_pinned() {
    let pins = PeerKeyPins::load(Arc::new(FullDisk)).await.unwrap();
    let refused = pins.pin_first(BOB, ALICE, KEY).await;
    assert_eq!(refused, Err("the disk is full".to_string()));
    assert_eq!(pins.pinned(BOB, ALICE), None);
}

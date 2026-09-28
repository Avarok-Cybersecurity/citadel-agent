//! One ILM per account: two over the same keys would each read-modify-write
//! the same queue blobs behind separate gates.
use super::support::{Loopback, MemoryDb};
use crate::kernel::ilm::host::{AgentIlmHost, AgentIlmRegistry, RegistryError};
use crate::kernel::ilm::kv::LocalDbAccess;
use citadel_sdk::prelude::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::watch;
use tokio::task::JoinHandle;

const CID: u64 = 77;

/// A LocalDB whose reads wait for `open`, so a start can be held mid-load.
#[derive(Clone)]
struct GatedDb {
    inner: MemoryDb,
    open: watch::Receiver<bool>,
    entered: Arc<AtomicBool>,
}

#[async_trait]
impl LocalDbAccess for GatedDb {
    async fn get(&self, cid: u64, key: &str) -> Result<Option<Vec<u8>>, String> {
        self.entered.store(true, Ordering::SeqCst);
        let mut open = self.open.clone();
        open.wait_for(|open| *open)
            .await
            .map_err(|e| e.to_string())?;
        self.inner.get(cid, key).await
    }

    async fn set(&self, cid: u64, key: &str, value: Vec<u8>) -> Result<(), String> {
        self.inner.set(cid, key, value).await
    }
}

fn gated(open: bool) -> (GatedDb, watch::Sender<bool>) {
    let (tx, rx) = watch::channel(open);
    let db = GatedDb {
        inner: MemoryDb::default(),
        open: rx,
        entered: Arc::new(AtomicBool::new(false)),
    };
    (db, tx)
}

#[tokio::test]
async fn a_second_ilm_for_one_account_is_refused() {
    let registry = AgentIlmRegistry::<GatedDb, Loopback>::default();
    let (db, _open) = gated(true);
    let (delivered, _rx) = unbounded_channel();

    let first = registry
        .start(CID, db.clone(), Loopback::default(), delivered.clone())
        .await
        .unwrap();
    let second = registry
        .start(CID, db.clone(), Loopback::default(), delivered.clone())
        .await;
    assert!(matches!(second, Err(RegistryError::AlreadyHosted(CID))));
    assert!(Arc::ptr_eq(&registry.get(CID).unwrap(), &first));

    assert!(registry.stop(CID).is_some());
    assert!(registry.get(CID).is_none());
    registry
        .start(CID, db, Loopback::default(), delivered)
        .await
        .expect("a stopped account can be hosted again");
}

type Started = Result<Arc<AgentIlmHost<GatedDb, Loopback>>, RegistryError>;

/// Holds a start inside its load until `open` is sent.
async fn held_start(
    registry: &Arc<AgentIlmRegistry<GatedDb, Loopback>>,
) -> (JoinHandle<Started>, watch::Sender<bool>) {
    let (held, open) = gated(false);
    let (delivered, delivered_rx) = unbounded_channel();
    let registry = registry.clone();
    let entered = held.entered.clone();
    let task = tokio::spawn(async move {
        let _inbox_stays_open = delivered_rx;
        registry
            .start(CID, held, Loopback::default(), delivered)
            .await
    });
    while !entered.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
    (task, open)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_start_cancelled_mid_load_does_not_resurrect() {
    let registry = Arc::new(AgentIlmRegistry::<GatedDb, Loopback>::default());

    // Stopped while loading, and hosted afresh while BOTH loads are in flight:
    // the first must not claim the second's reservation when it finishes.
    let (first, open_first) = held_start(&registry).await;
    assert!(registry.stop(CID).is_none(), "nothing was running yet");
    let (second, open_second) = held_start(&registry).await;

    open_first.send(true).unwrap();
    let first = first.await.unwrap();
    assert!(matches!(
        first,
        Err(RegistryError::StoppedWhileStarting(CID))
    ));
    assert!(registry.get(CID).is_none(), "the cancelled start went live");

    open_second.send(true).unwrap();
    let second = second.await.unwrap().expect("the fresh start was refused");
    assert!(Arc::ptr_eq(&registry.get(CID).unwrap(), &second));
}

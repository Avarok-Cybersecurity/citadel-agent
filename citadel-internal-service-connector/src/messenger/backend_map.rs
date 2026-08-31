//! Serialised read-modify-write over the whole-blob message maps.
//!
//! The inbound and outbound queues are each stored as ONE serialized blob under
//! a single LocalDB key. Every mutation is therefore read-whole, modify,
//! write-whole — and the four mutation sites did that with nothing holding the
//! two halves together. Two operations on the same map interleave like this:
//!
//! ```text
//!   store_outbound(m2)          clear_message_outbound(m1)
//!   read  -> {m1}
//!                               read  -> {m1}
//!   write -> {m1, m2}
//!                               write -> {}          <-- m2 is gone
//! ```
//!
//! `store_outbound` returned `Ok`, so ILM recorded the message as durably
//! queued. It was never sent, and because it is not in the queue it is never
//! retransmitted either: the send is silently lost. The window is a full
//! round trip to the agent and back (`wait_for_response` allows five seconds),
//! not a few instructions, so this is not a narrow race — a send concurrent
//! with an incoming ACK is ordinary traffic.
//!
//! The gate is per-map and per-backend instance. That matches the storage
//! granularity exactly: the key is `{prefix}-{cid}`, one backend is constructed
//! per CID, and clones share the `Arc`. It does NOT serialise across processes,
//! and cannot — two agents sharing one account's store would need a compare-and
//! -swap in the store itself. Nothing in this system does that today, and the
//! test below pins what this guard actually covers rather than what one might
//! hope it covers.
//!
//! The I/O is behind [`MapStore`] so the serialisation can be tested without a
//! running agent: a fake that yields between read and write reproduces the
//! interleave above deterministically.

use crate::messenger::WrappedMessage;
use async_trait::async_trait;
use citadel_io::tokio::sync::Mutex;
use intersession_layer_messaging::BackendError;
use std::collections::HashMap;
use uuid::Uuid;

/// `HashMap<peer_cid, HashMap<message_id, wrapped_message>>`
pub type State = HashMap<u64, HashMap<u64, WrappedMessage>>;

/// The two I/O halves of a map mutation, named so they can be faked.
#[async_trait]
pub trait MapStore: Sync {
    async fn read_map(&self, prefix: &str) -> Result<State, BackendError<WrappedMessage>>;
    async fn write_map(
        &self,
        prefix: &str,
        request_id: Uuid,
        state: State,
    ) -> Result<(), BackendError<WrappedMessage>>;
}

/// Read the map, apply `change`, write it back, with no other mutation of the
/// same map interleaving.
///
/// A failed read propagates rather than substituting an empty map: writing that
/// back would erase the whole queue, which is the same class of bug one level
/// down and is already guarded in `get_map`.
pub async fn mutate<S, F>(
    store: &S,
    gate: &Mutex<()>,
    prefix: &str,
    request_id: Uuid,
    change: F,
) -> Result<(), BackendError<WrappedMessage>>
where
    S: MapStore,
    F: FnOnce(&mut State) + Send,
{
    let _guard = gate.lock().await;
    let mut state = store.read_map(prefix).await?;
    change(&mut state);
    store.write_map(prefix, request_id, state).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use citadel_internal_service_types::{InternalServicePayload, InternalServiceResponse};
    use std::sync::Arc;

    const PREFIX: &str = "outbound_messages";

    /// A store whose read and write are separated by a yield, so any caller
    /// that does not hold a lock across both is guaranteed — not merely
    /// likely — to interleave with a concurrent one.
    #[derive(Default)]
    struct YieldingStore {
        state: std::sync::Mutex<State>,
    }

    #[async_trait]
    impl MapStore for YieldingStore {
        async fn read_map(&self, _prefix: &str) -> Result<State, BackendError<WrappedMessage>> {
            let snapshot = self.state.lock().unwrap().clone();
            // Stands in for the round trip to the agent.
            citadel_io::tokio::task::yield_now().await;
            Ok(snapshot)
        }

        async fn write_map(
            &self,
            _prefix: &str,
            _request_id: Uuid,
            state: State,
        ) -> Result<(), BackendError<WrappedMessage>> {
            citadel_io::tokio::task::yield_now().await;
            *self.state.lock().unwrap() = state;
            Ok(())
        }
    }

    fn message(id: u64) -> WrappedMessage {
        WrappedMessage {
            source_id: 1,
            destination_id: 2,
            message_id: id,
            contents: InternalServicePayload::Response(
                InternalServiceResponse::MessageSendSuccess(
                    citadel_internal_service_types::MessageSendSuccess {
                        cid: 1,
                        peer_cid: Some(2),
                        request_id: None,
                    },
                ),
            ),
        }
    }

    fn ids(state: &State) -> Vec<u64> {
        let mut out: Vec<u64> = state
            .values()
            .flat_map(|per_peer| per_peer.keys().copied())
            .collect();
        out.sort_unstable();
        out
    }

    /// The defect, stated as the property that was violated: an insert and a
    /// removal that run concurrently must both survive.
    #[citadel_io::tokio::test]
    async fn a_concurrent_insert_and_removal_both_survive() {
        let store = Arc::new(YieldingStore::default());
        store
            .state
            .lock()
            .unwrap()
            .entry(2)
            .or_default()
            .insert(1, message(1));

        let gate = Arc::new(Mutex::new(()));

        let inserting = {
            let (store, gate) = (store.clone(), gate.clone());
            citadel_io::tokio::spawn(async move {
                mutate(&*store, &gate, PREFIX, Uuid::new_v4(), |state| {
                    state.entry(2).or_default().insert(2, message(2));
                })
                .await
            })
        };
        let removing = {
            let (store, gate) = (store.clone(), gate.clone());
            citadel_io::tokio::spawn(async move {
                mutate(&*store, &gate, PREFIX, Uuid::new_v4(), |state| {
                    if let Some(per_peer) = state.get_mut(&2) {
                        per_peer.remove(&1);
                    }
                })
                .await
            })
        };

        inserting.await.unwrap().unwrap();
        removing.await.unwrap().unwrap();

        // Whichever order they ran in, the result is the same: 1 removed, 2
        // present. Without the gate the removal writes back a map read before
        // the insert, and 2 is gone — reported to its sender as queued.
        assert_eq!(ids(&store.state.lock().unwrap()), vec![2]);
    }

    /// Two inserts, because "the last writer wins" is the same defect with a
    /// different pair of operations and would otherwise go untested.
    #[citadel_io::tokio::test]
    async fn concurrent_inserts_do_not_overwrite_each_other() {
        let store = Arc::new(YieldingStore::default());
        let gate = Arc::new(Mutex::new(()));

        let mut handles = Vec::new();
        for id in 1..=8u64 {
            let (store, gate) = (store.clone(), gate.clone());
            handles.push(citadel_io::tokio::spawn(async move {
                mutate(&*store, &gate, PREFIX, Uuid::new_v4(), move |state| {
                    state.entry(2).or_default().insert(id, message(id));
                })
                .await
            }));
        }
        for handle in handles {
            handle.await.unwrap().unwrap();
        }

        assert_eq!(
            ids(&store.state.lock().unwrap()),
            (1..=8).collect::<Vec<_>>()
        );
    }

    /// A failed read must NOT be turned into an empty map and written back.
    /// That is the same erasure by another route, and the write must not happen
    /// at all.
    #[citadel_io::tokio::test]
    async fn a_failed_read_does_not_write_anything() {
        struct FailingRead {
            wrote: std::sync::atomic::AtomicBool,
        }

        #[async_trait]
        impl MapStore for FailingRead {
            async fn read_map(&self, _prefix: &str) -> Result<State, BackendError<WrappedMessage>> {
                Err(BackendError::StorageError("read failed".to_string()))
            }
            async fn write_map(
                &self,
                _prefix: &str,
                _request_id: Uuid,
                _state: State,
            ) -> Result<(), BackendError<WrappedMessage>> {
                self.wrote.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
        }

        let store = FailingRead {
            wrote: std::sync::atomic::AtomicBool::new(false),
        };
        let gate = Mutex::new(());
        let outcome = mutate(&store, &gate, PREFIX, Uuid::new_v4(), |_| {}).await;

        assert!(outcome.is_err(), "a failed read must not report success");
        assert!(
            !store.wrote.load(std::sync::atomic::Ordering::SeqCst),
            "a failed read still wrote the map"
        );
    }
}

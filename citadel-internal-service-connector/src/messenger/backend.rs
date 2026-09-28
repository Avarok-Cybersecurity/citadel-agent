use crate::messenger::backend_map::{MapStore, State};
use crate::messenger::backend_ws::WebSocketKvStore;
use crate::messenger::kv_store::IlmKvStore;
use crate::messenger::{MessengerTx, WrappedMessage};
use async_trait::async_trait;
use citadel_internal_service_types::InternalServiceResponse;
use citadel_io::tokio::sync::Mutex;
use intersession_layer_messaging::{Backend, BackendError};
use std::sync::Arc;
use uuid::Uuid;

/// ILM's durable state for one CID, over any [`IlmKvStore`].
///
/// Everything that decides what the state LOOKS like lives here and in
/// `backend_ilm.rs` / `backend_map.rs`: the `{prefix}-{cid}` keys, the bincode2
/// encoding of the queue maps, the gates. The store only moves bytes. That is
/// what lets the browser (`WebSocketKvStore`, the default) and the agent run
/// one implementation and write byte-identical state.
pub struct CitadelWorkspaceBackend<S: IlmKvStore = WebSocketKvStore> {
    pub cid: u64,
    pub(crate) store: Arc<S>,
    // Each map is one serialized blob under one key, so every mutation is a
    // read-whole/modify/write-whole. Two of them interleaving lose one of the
    // two changes -- and the lost one was reported `Ok`. Held across read AND
    // write; see messenger/backend_map.rs for the interleave and its limits.
    // Separate gates because the two maps are separate keys and never mutate
    // together.
    pub(crate) outbound_gate: Arc<Mutex<()>>,
    pub(crate) inbound_gate: Arc<Mutex<()>>,
}

impl<S: IlmKvStore> Clone for CitadelWorkspaceBackend<S> {
    fn clone(&self) -> Self {
        Self {
            cid: self.cid,
            store: self.store.clone(),
            outbound_gate: self.outbound_gate.clone(),
            inbound_gate: self.inbound_gate.clone(),
        }
    }
}

// Constants for storage prefixes
pub const INBOUND_MESSAGE_PREFIX: &str = "inbound_messages";
pub const OUTBOUND_MESSAGE_PREFIX: &str = "outbound_messages";

impl<S: IlmKvStore> CitadelWorkspaceBackend<S> {
    /// One backend per CID: the gates only serialise writers that share them.
    pub fn with_store(cid: u64, store: S) -> Self {
        Self {
            cid,
            store: Arc::new(store),
            outbound_gate: Arc::new(Mutex::new(())),
            inbound_gate: Arc::new(Mutex::new(())),
        }
    }

    /// The key every ILM value is stored under. The browser's ILM has always
    /// written `{key}-{cid}`; an agent-hosted ILM must read the same key to
    /// take that state over, so this is the ONE place it is formed.
    pub fn storage_key(&self, key: &str) -> String {
        format!("{}-{}", key, self.cid)
    }

    /// Generic function to get a map (inbound or outbound)
    pub async fn get_map(&self, prefix: &str) -> Result<State, BackendError<WrappedMessage>> {
        // A failed or timed-out read is an Err from the store, NOT an empty
        // map. Every caller here is a read-modify-write over the WHOLE queue,
        // so treating a slow read as "empty" replaced the pending queue with a
        // map holding only the new message -- silently erasing every other
        // queued message, each of whose senders had already been shown "sent".
        // Genuine absence (`Ok(None)`) still initialises.
        match self.store.get(&self.storage_key(prefix)).await? {
            Some(bytes) => {
                citadel_logging::debug!(target: "citadel", "[GET_MAP] Got {} map successfully", prefix);
                bincode2::deserialize(&bytes).map_err(|err| {
                    BackendError::StorageError(format!("Failed to deserialize {prefix} map: {err}"))
                })
            }
            None => {
                citadel_logging::debug!(target: "citadel", "[GET_MAP] {} map not found, initializing new one", prefix);
                self.initialize_map(prefix).await
            }
        }
    }

    /// Generic function to initialize a map (inbound or outbound)
    async fn initialize_map(&self, prefix: &str) -> Result<State, BackendError<WrappedMessage>> {
        let new_state = State::new();
        self.update_map(prefix, Uuid::new_v4(), new_state.clone())
            .await?;
        citadel_logging::debug!(target: "citadel", "[INITIALIZE_MAP] Initialized {} map successfully", prefix);
        Ok(new_state)
    }

    /// Generic function to update a map (inbound or outbound)
    pub async fn update_map(
        &self,
        prefix: &str,
        request_id: Uuid,
        state: State,
    ) -> Result<(), BackendError<WrappedMessage>> {
        let value = bincode2::serialize(&state).map_err(|err| {
            BackendError::StorageError(format!("Failed to serialize {prefix} map: {err}"))
        })?;

        self.store
            .set(request_id, &self.storage_key(prefix), value)
            .await
            .inspect(|_| {
                citadel_logging::debug!(target: "citadel", "[UPDATE_MAP] Updated {} map successfully", prefix);
            })
    }

    // There is deliberately no `update_inbound_map` / `update_outbound_map`
    // convenience pair any more. They existed only to be called right after
    // `get_*_map`, and that read-then-write with nothing between them holding
    // the two halves together IS the lost-update bug. `backend_map::mutate` is
    // now the only way to write either map, so a future caller cannot
    // reconstruct the unsynchronised sequence without noticing.
}

/// The two I/O halves `backend_map::mutate` drives. Thin wrappers over the
/// existing generic map functions, named separately so the serialisation can be
/// tested against a fake instead of a running agent.
#[async_trait]
impl<S: IlmKvStore> MapStore for CitadelWorkspaceBackend<S> {
    async fn read_map(&self, prefix: &str) -> Result<State, BackendError<WrappedMessage>> {
        self.get_map(prefix).await
    }

    async fn write_map(
        &self,
        prefix: &str,
        request_id: Uuid,
        state: State,
    ) -> Result<(), BackendError<WrappedMessage>> {
        self.update_map(prefix, request_id, state).await
    }
}

#[async_trait]
pub trait CitadelBackendExt: Backend<WrappedMessage> + Clone + Send + Sync + 'static {
    /// Creates a new instance of the backend
    async fn new(
        cid: u64,
        handle: &MessengerTx<Self>,
    ) -> Result<Self, BackendError<WrappedMessage>>;

    /// Inspects a payload to see if it is relevant to the backend. If it is, the response
    /// is not returned. Otherwise, the response is returned to the caller for further processing.
    async fn inspect_received_payload(
        &self,
        response: InternalServiceResponse,
    ) -> Result<Option<InternalServiceResponse>, BackendError<WrappedMessage>> {
        Ok(Some(response))
    }
}

#[async_trait]
impl CitadelBackendExt for CitadelWorkspaceBackend<WebSocketKvStore> {
    async fn new(
        cid: u64,
        handle: &MessengerTx<Self>,
    ) -> Result<Self, BackendError<WrappedMessage>> {
        Ok(Self::with_store(
            cid,
            WebSocketKvStore::new(cid, handle.bypass_ism_outbound_tx.clone()),
        ))
    }

    async fn inspect_received_payload(
        &self,
        response: InternalServiceResponse,
    ) -> Result<Option<InternalServiceResponse>, BackendError<WrappedMessage>> {
        citadel_logging::debug!(target: "citadel", "Inspecting received payload: {:?}", response);
        Ok(self.store.claim_response(response))
    }
}

#[cfg(test)]
#[path = "backend_ws_tests.rs"]
mod ws_request_tests;

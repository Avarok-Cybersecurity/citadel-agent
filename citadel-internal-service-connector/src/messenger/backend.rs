use crate::messenger::backend_map::{mutate, MapStore, State};
use crate::messenger::{sleep_internal, timeout_internal, BypasserTx, MessengerTx, WrappedMessage};
use async_trait::async_trait;
use citadel_internal_service_types::{
    BatchedResponseData, InternalServicePayload, InternalServiceRequest, InternalServiceResponse,
};
use citadel_io::tokio::sync::Mutex;
use dashmap::DashMap;
use intersession_layer_messaging::{Backend, BackendError};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone)]
pub struct CitadelWorkspaceBackend {
    pub cid: u64,
    expected_requests:
        Arc<DashMap<Uuid, citadel_io::tokio::sync::oneshot::Sender<InternalServiceResponse>>>,
    bypass_ism_outbound_tx: Option<BypasserTx>,
    // Each map is one serialized blob under one key, so every mutation is a
    // read-whole/modify/write-whole. Two of them interleaving lose one of the
    // two changes -- and the lost one was reported `Ok`. Held across read AND
    // write; see messenger/backend_map.rs for the interleave and its limits.
    // Separate gates because the two maps are separate keys and never mutate
    // together.
    outbound_gate: Arc<Mutex<()>>,
    inbound_gate: Arc<Mutex<()>>,
}

// Constants for storage prefixes
pub const INBOUND_MESSAGE_PREFIX: &str = "inbound_messages";
pub const OUTBOUND_MESSAGE_PREFIX: &str = "outbound_messages";

impl CitadelWorkspaceBackend {
    async fn wait_for_response(&self, request_id: Uuid) -> Option<InternalServiceResponse> {
        let (tx, rx) = citadel_io::tokio::sync::oneshot::channel();
        self.expected_requests.insert(request_id, tx);
        citadel_logging::info!(target: "citadel", "[BACKEND-WAIT] Waiting for response to request_id: {} (CID: {})", request_id, self.cid);

        // Add a timeout to prevent infinite waiting (using platform-agnostic timeout)
        match timeout_internal(Duration::from_secs(5), rx).await {
            Ok(result) => {
                let response = result.ok();
                citadel_logging::info!(target: "citadel", "[BACKEND-WAIT] Received response for request_id {}: {:?}", request_id, response.as_ref().map(|r| std::any::type_name_of_val(r)));
                response
            }
            Err(_) => {
                // Remove the request from expected_requests if it times out
                self.expected_requests.remove(&request_id);
                citadel_logging::warn!(target: "citadel", "[BACKEND-WAIT] TIMEOUT waiting for response to request_id: {} (CID: {}, pending requests: {})",
                    request_id, self.cid, self.expected_requests.len());
                None
            }
        }
    }

    /// Sends a message to the network layer
    pub async fn send_to_network(
        &self,
        request: InternalServiceRequest,
    ) -> Result<(), BackendError<WrappedMessage>> {
        citadel_logging::info!(target: "citadel", "[BACKEND-NETWORK] send_to_network called for CID {} with request: {:?}", self.cid, std::any::type_name_of_val(&request));
        // Send the message to the network layer
        if let Some(tx) = &self.bypass_ism_outbound_tx {
            tx.send(request).await.map_err(|err| {
                citadel_logging::error!(target: "citadel", "[BACKEND-NETWORK] Failed to send bypass message: {}", err);
                BackendError::StorageError(format!("Failed to send bypass message: {err}"))
            })?;
            citadel_logging::info!(target: "citadel", "[BACKEND-NETWORK] Successfully sent to bypass channel");
        } else {
            citadel_logging::error!(target: "citadel", "[BACKEND-NETWORK] bypass_ism_outbound_tx is None!");
            return Err(BackendError::StorageError(
                "Failed to send bypass message: bypass_ism_outbound_tx is None".to_string(),
            ));
        }

        Ok(())
    }

    /// Generic function to get a map (inbound or outbound)
    pub async fn get_map(&self, prefix: &str) -> Result<State, BackendError<WrappedMessage>> {
        let request_id = Uuid::new_v4();
        let key = format!("{}-{}", prefix, self.cid);

        let request = InternalServiceRequest::LocalDBGetKV {
            request_id,
            cid: self.cid,
            peer_cid: None,
            key,
        };

        self.send_to_network(request).await?;

        if let Some(response) = self.wait_for_response(request_id).await {
            match response {
                InternalServiceResponse::LocalDBGetKVSuccess(success_response) => {
                    citadel_logging::debug!(target: "citadel", "[GET_MAP] Got {} map successfully", prefix);
                    let state: State =
                        bincode2::deserialize(&success_response.value).map_err(|err| {
                            BackendError::StorageError(format!(
                                "Failed to deserialize {prefix} map: {err}"
                            ))
                        })?;
                    Ok(state)
                }
                InternalServiceResponse::LocalDBGetKVFailure(failure_response) => {
                    let failure_message = failure_response.message;
                    if failure_message == "Key not found" {
                        citadel_logging::debug!(target: "citadel", "[GET_MAP] {} map not found, initializing new one", prefix);
                        self.initialize_map(prefix).await
                    } else {
                        Err(BackendError::StorageError(format!(
                            "Failed to get {prefix} map: {failure_message}"
                        )))
                    }
                }
                _ => Err(BackendError::StorageError(format!(
                    "Unexpected response when getting {prefix} map"
                ))),
            }
        } else {
            // A timeout is NOT "the map is empty".
            //
            // This used to return an empty map, and every caller here is a
            // read-modify-write over the WHOLE queue: get the map, change one
            // entry, write it back. So one slow LocalDB read during a send
            // replaced the entire pending queue with a map containing only the
            // new message — silently erasing every other queued message, each of
            // whose senders had already been shown "sent". Genuine absence is a
            // different answer ("Key not found", handled above) and still
            // initializes.
            Err(BackendError::StorageError(format!(
                "Timed out reading the {prefix} map; refusing to treat that as an empty queue"
            )))
        }
    }

    /// Generic function to initialize a map (inbound or outbound)
    async fn initialize_map(&self, prefix: &str) -> Result<State, BackendError<WrappedMessage>> {
        let request_id = Uuid::new_v4();
        let key = format!("{}-{}", prefix, self.cid);
        let new_state = State::new();

        let value = bincode2::serialize(&new_state).map_err(|err| {
            BackendError::StorageError(format!("Failed to serialize {prefix} map: {err}"))
        })?;

        let request = InternalServiceRequest::LocalDBSetKV {
            request_id,
            cid: self.cid,
            peer_cid: None,
            key,
            value,
        };

        self.send_to_network(request).await?;

        if let Some(response) = self.wait_for_response(request_id).await {
            if let InternalServiceResponse::LocalDBSetKVSuccess(_) = response {
                citadel_logging::debug!(target: "citadel", "[INITIALIZE_MAP] Initialized {} map successfully", prefix);
                Ok(new_state)
            } else {
                Err(BackendError::StorageError(format!(
                    "Failed to initialize {prefix} map"
                )))
            }
        } else {
            // Not "assume it worked". Handing back an empty State on an
            // unacknowledged write says the map is initialised when the key may
            // not exist, and the caller then treats an empty queue as fact.
            // `update_map` two functions below already refuses to do this; the
            // two had drifted apart.
            Err(BackendError::StorageError(format!(
                "Timed out initializing the {prefix} map; it may not exist"
            )))
        }
    }

    /// Generic function to update a map (inbound or outbound)
    pub async fn update_map(
        &self,
        prefix: &str,
        request_id: Uuid,
        state: State,
    ) -> Result<(), BackendError<WrappedMessage>> {
        let key = format!("{}-{}", prefix, self.cid);

        let value = bincode2::serialize(&state).map_err(|err| {
            BackendError::StorageError(format!("Failed to serialize {prefix} map: {err}"))
        })?;

        let request = InternalServiceRequest::LocalDBSetKV {
            request_id,
            cid: self.cid,
            peer_cid: None,
            key,
            value,
        };

        self.send_to_network(request).await?;

        if self.wait_for_response(request_id).await.is_some() {
            citadel_logging::debug!(target: "citadel", "[UPDATE_MAP] Updated {} map successfully", prefix);
            Ok(())
        } else {
            // Reporting success for a write we never saw acknowledged tells the
            // sender their message is durably queued when it may not be. Fail,
            // so the caller marks it failed and the user can retry — a visible
            // failure beats a checkmark on a message that is gone.
            Err(BackendError::StorageError(format!(
                "Timed out writing the {prefix} map; the change may not be stored"
            )))
        }
    }

    // Convenience methods that use the generic functions
    async fn get_inbound_map(&self) -> Result<State, BackendError<WrappedMessage>> {
        self.get_map(INBOUND_MESSAGE_PREFIX).await
    }

    async fn get_outbound_map(&self) -> Result<State, BackendError<WrappedMessage>> {
        self.get_map(OUTBOUND_MESSAGE_PREFIX).await
    }

    // There is deliberately no `update_inbound_map` / `update_outbound_map`
    // convenience pair any more. They existed only to be called right after
    // `get_*_map`, and that read-then-write with nothing between them holding
    // the two halves together IS the lost-update bug. `backend_map::mutate` is
    // now the only way to write either map, so a future caller cannot
    // reconstruct the unsynchronised sequence without noticing.

    pub fn add_expected_request(&self, request_id: Uuid) {
        let (tx, _rx) = citadel_io::tokio::sync::oneshot::channel();
        self.expected_requests.insert(request_id, tx);
    }

    /// Sends multiple requests in a single batch and waits for all responses.
    /// This is more efficient than sequential requests as it:
    /// 1. Uses a single network roundtrip
    /// 2. Backend executes all requests in parallel
    /// 3. Avoids sequential await blocking in WASM
    ///
    /// Returns responses in the same order as the input requests.
    pub async fn send_batched(
        &self,
        requests: Vec<InternalServiceRequest>,
    ) -> Result<Vec<InternalServiceResponse>, BackendError<WrappedMessage>> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }

        let batch_request_id = Uuid::new_v4();
        citadel_logging::info!(target: "citadel", "[SEND_BATCHED] Sending {} requests in batch, request_id={}", requests.len(), batch_request_id);

        let batched_request = InternalServiceRequest::Batched {
            request_id: batch_request_id,
            commands: requests,
        };

        self.send_to_network(batched_request).await?;

        if let Some(response) = self.wait_for_response(batch_request_id).await {
            match response {
                InternalServiceResponse::BatchedResponse(BatchedResponseData {
                    results, ..
                }) => Ok(results),
                other => {
                    citadel_logging::warn!(target: "citadel", "[SEND_BATCHED] Unexpected response type: {:?}", other);
                    Err(BackendError::StorageError(
                        "Unexpected response type for batched request".to_string(),
                    ))
                }
            }
        } else {
            citadel_logging::warn!(target: "citadel", "[SEND_BATCHED] Timeout waiting for batched response");
            Err(BackendError::StorageError(
                "Timeout waiting for batched response".to_string(),
            ))
        }
    }

    /// Loads multiple values in a single batched request.
    /// More efficient than calling load_value() multiple times.
    pub async fn load_values_batched(
        &self,
        keys: &[&str],
    ) -> Result<Vec<Option<Vec<u8>>>, BackendError<WrappedMessage>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }

        // Build batch of LocalDBGetKV requests
        let requests: Vec<InternalServiceRequest> = keys
            .iter()
            .map(|key| InternalServiceRequest::LocalDBGetKV {
                request_id: Uuid::new_v4(),
                cid: self.cid,
                peer_cid: None,
                key: format!("{}-{}", key, self.cid),
            })
            .collect();

        let responses = self.send_batched(requests).await?;

        // Extract values from responses
        let results: Vec<Option<Vec<u8>>> = responses
            .into_iter()
            .map(|resp| match resp {
                InternalServiceResponse::LocalDBGetKVSuccess(success) => Some(success.value),
                _ => None,
            })
            .collect();

        Ok(results)
    }
}

/// The two I/O halves `backend_map::mutate` drives. Thin wrappers over the
/// existing generic map functions, named separately so the serialisation can be
/// tested against a fake instead of a running agent.
#[async_trait]
impl MapStore for CitadelWorkspaceBackend {
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
impl Backend<WrappedMessage> for CitadelWorkspaceBackend {
    async fn store_outbound(
        &self,
        message: WrappedMessage,
    ) -> Result<(), BackendError<WrappedMessage>> {
        let message_id = message.message_id;
        let peer_cid = message.destination_id;
        let request_id = if let InternalServicePayload::Request(request) = &message.contents {
            request.request_id().copied().unwrap_or_default()
        } else {
            Uuid::new_v4()
        };

        citadel_logging::debug!(target: "citadel", "[STORE_OUTBOUND] Storing outbound message: source_id={}, destination_id={}, message_id={}",
            message.source_id, message.destination_id, message.message_id);

        mutate(
            self,
            &self.outbound_gate,
            OUTBOUND_MESSAGE_PREFIX,
            request_id,
            move |outbound| {
                outbound
                    .entry(peer_cid)
                    .or_insert_with(HashMap::new)
                    .insert(message_id, message);
            },
        )
        .await
    }

    async fn store_inbound(
        &self,
        message: WrappedMessage,
    ) -> Result<(), BackendError<WrappedMessage>> {
        let message_id = message.message_id;
        let peer_cid = message.source_id; // Use source_id for inbound messages
        let request_id = if let InternalServicePayload::Request(request) = &message.contents {
            request.request_id().copied().unwrap_or_default()
        } else {
            Uuid::new_v4()
        };

        citadel_logging::debug!(target: "citadel", "[STORE_INBOUND] Storing inbound message: source_id={}, destination_id={}, message_id={}",
            message.source_id, message.destination_id, message.message_id);

        mutate(
            self,
            &self.inbound_gate,
            INBOUND_MESSAGE_PREFIX,
            request_id,
            move |inbound| {
                inbound
                    .entry(peer_cid)
                    .or_insert_with(HashMap::new)
                    .insert(message_id, message);
            },
        )
        .await
    }

    async fn clear_message_inbound(
        &self,
        peer_id: u64,
        message_id: u64,
    ) -> Result<(), BackendError<WrappedMessage>> {
        mutate(
            self,
            &self.inbound_gate,
            INBOUND_MESSAGE_PREFIX,
            Uuid::new_v4(),
            move |inbound| {
                if let Some(peer_messages) = inbound.get_mut(&peer_id) {
                    peer_messages.remove(&message_id);
                }
            },
        )
        .await
    }

    async fn clear_message_outbound(
        &self,
        peer_id: u64,
        message_id: u64,
    ) -> Result<(), BackendError<WrappedMessage>> {
        mutate(
            self,
            &self.outbound_gate,
            OUTBOUND_MESSAGE_PREFIX,
            Uuid::new_v4(),
            move |outbound| {
                if let Some(peer_messages) = outbound.get_mut(&peer_id) {
                    peer_messages.remove(&message_id);
                }
            },
        )
        .await
    }

    /// One read-modify-write for the whole set.
    ///
    /// Acknowledgement is cumulative, so a single ACK routinely retires a whole
    /// send window. Clearing them one at a time meant a full queue read AND a
    /// full queue write per covered id: O(window) round trips to the agent and
    /// O(window^2) bytes serialised, for one ACK.
    async fn clear_messages_outbound(
        &self,
        peer_id: u64,
        message_ids: &[u64],
    ) -> Result<(), BackendError<WrappedMessage>> {
        if message_ids.is_empty() {
            return Ok(());
        }
        let message_ids = message_ids.to_vec();
        mutate(
            self,
            &self.outbound_gate,
            OUTBOUND_MESSAGE_PREFIX,
            Uuid::new_v4(),
            move |outbound| {
                if let Some(peer_messages) = outbound.get_mut(&peer_id) {
                    for message_id in &message_ids {
                        peer_messages.remove(message_id);
                    }
                }
            },
        )
        .await
    }

    async fn get_pending_outbound(
        &self,
    ) -> Result<Vec<WrappedMessage>, BackendError<WrappedMessage>> {
        loop {
            match self.get_outbound_map().await {
                Ok(outbound) => {
                    return Ok(outbound
                        .values()
                        .flat_map(|messages| messages.values().cloned())
                        .collect())
                }
                Err(e) => {
                    // If we get a delivery error, log it and return an empty vector
                    let err_str = format!("{e:?}");
                    if err_str.contains("Failed to deliver message")
                        || err_str.contains("get_kv: Server connection not found")
                    {
                        citadel_logging::warn!(target: "citadel", "[GET_PENDING_OUTBOUND] Failed to get outbound map due to likely no connection up yet");
                        sleep_internal(Duration::from_millis(5000)).await;
                        continue;
                    } else {
                        return Err(e);
                    }
                }
            }
        }
    }

    async fn get_pending_inbound(
        &self,
    ) -> Result<Vec<WrappedMessage>, BackendError<WrappedMessage>> {
        loop {
            match self.get_inbound_map().await {
                Ok(inbound) => {
                    return Ok(inbound
                        .values()
                        .flat_map(|messages| messages.values().cloned())
                        .collect())
                }
                Err(e) => {
                    // If we get a delivery error, log it and return an empty vector
                    let err_str = format!("{e:?}");
                    if err_str.contains("Failed to deliver message")
                        || err_str.contains("get_kv: Server connection not found")
                    {
                        citadel_logging::warn!(target: "citadel", "[GET_PENDING_INBOUND] Failed to get inbound map likely due to likely no connection up yet");
                        sleep_internal(Duration::from_millis(5000)).await;
                        continue;
                    } else {
                        return Err(e);
                    }
                }
            }
        }
    }

    async fn store_value(
        &self,
        key: &str,
        value: &[u8],
    ) -> Result<(), BackendError<WrappedMessage>> {
        let request_id = Uuid::new_v4();
        let unique_key = format!("{}-{}", key, self.cid);

        let request = InternalServiceRequest::LocalDBSetKV {
            request_id,
            cid: self.cid,
            peer_cid: None,
            key: unique_key,
            value: value.to_vec(),
        };

        self.send_to_network(request).await?;

        if self.wait_for_response(request_id).await.is_some() {
            citadel_logging::debug!(target: "citadel", "[STORE_VALUE] Stored value for key={}", key);
            Ok(())
        } else {
            // Not "assume it worked". This is how the delivery frontier and
            // the next-id counter are persisted, and a silent loss of either is
            // what turns a reconnect into re-minted ids the receiver swallows
            // as duplicates. A caller told Ok has no reason to retry.
            Err(BackendError::StorageError(format!(
                "Timed out storing the value for key={key}; it may not be stored"
            )))
        }
    }

    async fn load_value(&self, key: &str) -> Result<Option<Vec<u8>>, BackendError<WrappedMessage>> {
        let request_id = Uuid::new_v4();
        let unique_key = format!("{}-{}", key, self.cid);

        let request = InternalServiceRequest::LocalDBGetKV {
            request_id,
            cid: self.cid,
            peer_cid: None,
            key: unique_key,
        };

        self.send_to_network(request).await?;

        if let Some(response) = self.wait_for_response(request_id).await {
            citadel_logging::debug!(target: "citadel", "[LOAD_VALUE] Loaded value for key={}", key);
            match response {
                InternalServiceResponse::LocalDBGetKVSuccess(success) => Ok(Some(success.value)),
                _ => Ok(None),
            }
        } else {
            // If we get no response, assume the key doesn't exist
            citadel_logging::warn!(target: "citadel", "[LOAD_VALUE] No response received when loading value for key={}, assuming key doesn't exist", key);
            Ok(None)
        }
    }

    async fn load_values_batched(
        &self,
        keys: &[&str],
    ) -> Result<Vec<Option<Vec<u8>>>, BackendError<WrappedMessage>> {
        // Delegate to the inherent method that uses batched network requests
        CitadelWorkspaceBackend::load_values_batched(self, keys).await
    }

    /// One round trip for the whole set, mirroring `load_values_batched`.
    ///
    /// The inbound path writes the receipt map and the per-peer high-water mark
    /// for every arriving message, inline in the single sequential listener.
    /// Two separate `store_value` calls meant two round trips to the agent per
    /// message, each with its own five-second `wait_for_response` window in
    /// which one lost response freezes ALL inbound processing -- ACKs included,
    /// so the senders start retransmitting into a receiver that is not reading.
    async fn store_values_batched(
        &self,
        entries: &[(&str, Vec<u8>)],
    ) -> Result<(), BackendError<WrappedMessage>> {
        if entries.is_empty() {
            return Ok(());
        }

        let requests: Vec<InternalServiceRequest> = entries
            .iter()
            .map(|(key, value)| InternalServiceRequest::LocalDBSetKV {
                request_id: Uuid::new_v4(),
                cid: self.cid,
                peer_cid: None,
                key: format!("{}-{}", key, self.cid),
                value: value.clone(),
            })
            .collect();

        let responses = self.send_batched(requests).await?;

        // A missing or failed acknowledgement is a failure, not a silence to
        // step over: `update_map` and `store_value` both refuse to report an
        // unacknowledged write as success, and this must agree with them.
        for (index, response) in responses.iter().enumerate() {
            if !matches!(response, InternalServiceResponse::LocalDBSetKVSuccess(_)) {
                let key = entries[index].0;
                return Err(BackendError::StorageError(format!(
                    "Batched store for key={key} was not acknowledged: {response:?}"
                )));
            }
        }
        if responses.len() != entries.len() {
            return Err(BackendError::StorageError(format!(
                "Batched store expected {} acknowledgements, got {}",
                entries.len(),
                responses.len()
            )));
        }
        Ok(())
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
impl CitadelBackendExt for CitadelWorkspaceBackend {
    async fn new(
        cid: u64,
        handle: &MessengerTx<Self>,
    ) -> Result<Self, BackendError<WrappedMessage>> {
        Ok(Self {
            cid,
            expected_requests: Arc::new(DashMap::new()),
            bypass_ism_outbound_tx: Some(handle.bypass_ism_outbound_tx.clone()),
            outbound_gate: Arc::new(Mutex::new(())),
            inbound_gate: Arc::new(Mutex::new(())),
        })
    }

    async fn inspect_received_payload(
        &self,
        response: InternalServiceResponse,
    ) -> Result<Option<InternalServiceResponse>, BackendError<WrappedMessage>> {
        citadel_logging::debug!(target: "citadel", "Inspecting received payload: {:?}", response);

        if let Some(id) = response.request_id() {
            if let Some(tx) = self.expected_requests.remove(id) {
                let _ = tx.1.send(response.clone());
                return Ok(None);
            }
        }

        Ok(Some(response))
    }
}

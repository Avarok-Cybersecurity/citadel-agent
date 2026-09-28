//! The browser's ILM storage: LocalDB requests over the agent's WebSocket.
//!
//! This is the storage half of what `CitadelWorkspaceBackend` did before it
//! became storage-agnostic, moved rather than rewritten: the same requests
//! (`LocalDBGetKV` / `LocalDBSetKV` / `Batched`, always `peer_cid: None`), the
//! same five-second wait per request, and the same classification of answers
//! (`backend_outcome.rs`). `backend_ws_tests.rs` pins the request sequence.
use crate::messenger::backend_outcome::{read_outcome, write_outcome};
use crate::messenger::kv_store::{IlmKvStore, KvResult};
use crate::messenger::{timeout_internal, BypasserTx};
use async_trait::async_trait;
use citadel_internal_service_types::{
    BatchedResponseData, InternalServiceRequest, InternalServiceResponse,
};
use citadel_io::tokio::sync::oneshot;
use dashmap::DashMap;
use intersession_layer_messaging::BackendError;
use std::time::Duration;
use uuid::Uuid;

pub struct WebSocketKvStore {
    cid: u64,
    expected_requests: DashMap<Uuid, oneshot::Sender<InternalServiceResponse>>,
    outbound: BypasserTx,
}

impl WebSocketKvStore {
    pub fn new(cid: u64, outbound: BypasserTx) -> Self {
        Self {
            cid,
            expected_requests: DashMap::new(),
            outbound,
        }
    }

    async fn wait_for_response(&self, request_id: Uuid) -> Option<InternalServiceResponse> {
        let (tx, rx) = oneshot::channel();
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
    pub async fn send_to_network(&self, request: InternalServiceRequest) -> KvResult<()> {
        citadel_logging::info!(target: "citadel", "[BACKEND-NETWORK] send_to_network called for CID {} with request: {:?}", self.cid, std::any::type_name_of_val(&request));
        self.outbound.send(request).await.map_err(|err| {
            citadel_logging::error!(target: "citadel", "[BACKEND-NETWORK] Failed to send bypass message: {}", err);
            BackendError::StorageError(format!("Failed to send bypass message: {err}"))
        })?;
        citadel_logging::info!(target: "citadel", "[BACKEND-NETWORK] Successfully sent to bypass channel");
        Ok(())
    }

    pub fn add_expected_request(&self, request_id: Uuid) {
        let (tx, _rx) = oneshot::channel();
        self.expected_requests.insert(request_id, tx);
    }

    /// Claims a response this store is waiting for. `None` means it was
    /// consumed; `Some` hands it back for normal processing.
    pub fn claim_response(
        &self,
        response: InternalServiceResponse,
    ) -> Option<InternalServiceResponse> {
        if let Some(id) = response.request_id() {
            if let Some((_, tx)) = self.expected_requests.remove(id) {
                let _ = tx.send(response);
                return None;
            }
        }
        Some(response)
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
    ) -> KvResult<Vec<InternalServiceResponse>> {
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
}

#[async_trait]
impl IlmKvStore for WebSocketKvStore {
    async fn get(&self, key: &str) -> KvResult<Option<Vec<u8>>> {
        let request_id = Uuid::new_v4();
        let request = InternalServiceRequest::LocalDBGetKV {
            request_id,
            cid: self.cid,
            peer_cid: None,
            key: key.to_string(),
        };

        self.send_to_network(request).await?;

        // A timeout is NOT absence. Every map caller is a read-modify-write of
        // the WHOLE queue, and the tracker reads its delivery frontier and
        // next-id counter this way: "nothing stored" on a slow read erased
        // queues and re-delivered messages. See `read_outcome` for the rest.
        match self.wait_for_response(request_id).await {
            Some(response) => read_outcome(response, key),
            None => Err(BackendError::StorageError(format!(
                "Timed out reading key={key}; whether it exists is unknown"
            ))),
        }
    }

    async fn set(&self, request_id: Uuid, key: &str, value: Vec<u8>) -> KvResult<()> {
        let request = InternalServiceRequest::LocalDBSetKV {
            request_id,
            cid: self.cid,
            peer_cid: None,
            key: key.to_string(),
            value,
        };

        self.send_to_network(request).await?;

        write_outcome(
            self.wait_for_response(request_id).await,
            &format!("key={key}"),
        )
    }

    async fn get_many(&self, keys: &[String]) -> KvResult<Vec<Option<Vec<u8>>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }

        let requests: Vec<InternalServiceRequest> = keys
            .iter()
            .map(|key| InternalServiceRequest::LocalDBGetKV {
                request_id: Uuid::new_v4(),
                cid: self.cid,
                peer_cid: None,
                key: key.clone(),
            })
            .collect();

        let responses = self.send_batched(requests).await?;

        // Positional mapping needs the positions to line up.
        //
        // The agent builds a batch response with `filter_map`, dropping any
        // sub-command whose handler answered nothing, so a short array is
        // reachable -- and every value after the gap would then be attributed
        // to the WRONG KEY. `MessageTracker::new` loads six keys, five of them
        // `HashMap<u64, u64>`: `last_acked` deserialises perfectly from
        // `last_sent`'s bytes, and nothing anywhere reports a fault.
        if responses.len() != keys.len() {
            return Err(BackendError::StorageError(format!(
                "Batched read expected {} results, got {}; the remaining values would be \
                 attributed to the wrong keys",
                keys.len(),
                responses.len()
            )));
        }

        let mut results: Vec<Option<Vec<u8>>> = Vec::with_capacity(responses.len());
        for (index, resp) in responses.into_iter().enumerate() {
            results.push(read_outcome(resp, &keys[index])?);
        }

        Ok(results)
    }

    async fn set_many(&self, entries: &[(String, Vec<u8>)]) -> KvResult<()> {
        if entries.is_empty() {
            return Ok(());
        }

        let requests: Vec<InternalServiceRequest> = entries
            .iter()
            .map(|(key, value)| InternalServiceRequest::LocalDBSetKV {
                request_id: Uuid::new_v4(),
                cid: self.cid,
                peer_cid: None,
                key: key.clone(),
                value: value.clone(),
            })
            .collect();

        let responses = self.send_batched(requests).await?;

        // A missing or failed acknowledgement is a failure, not a silence to
        // step over: `set` refuses to report an unacknowledged write as
        // success, and this must agree with it.
        for (index, response) in responses.iter().enumerate() {
            if !matches!(response, InternalServiceResponse::LocalDBSetKVSuccess(_)) {
                let key = &entries[index].0;
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

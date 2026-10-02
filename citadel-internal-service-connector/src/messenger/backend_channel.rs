//! How a backend's LocalDB requests reach the agent.
//!
//! The browser (and any other client of the agent's socket) sends each request
//! over its connection and waits for the reply carrying its id: [`RequestChannel`].
//! The agent, hosting ILM itself, answers the same requests in-process with its
//! own channel. Both feed one [`CitadelWorkspaceBackend`](super::backend::CitadelWorkspaceBackend),
//! so the storage logic exists once.

use crate::messenger::{BypasserTx, WrappedMessage};
use async_trait::async_trait;
use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use intersession_layer_messaging::BackendError;
use std::sync::Arc;
use uuid::Uuid;

/// Answers one LocalDB (or Batched) request.
///
/// `Ok` is the agent's answer, whatever it says -- a refusal is an answer, and
/// the backend classifies it. `Err` means no answer will come.
#[async_trait]
pub trait BackendChannel: Clone + Send + Sync + 'static {
    async fn request(
        &self,
        request: InternalServiceRequest,
        request_id: Uuid,
    ) -> Result<InternalServiceResponse, BackendError<WrappedMessage>>;
}

type Waiters =
    Arc<DashMap<Uuid, citadel_io::tokio::sync::oneshot::Sender<InternalServiceResponse>>>;

/// Requests over a client's socket to the agent, matched to replies by id.
#[derive(Clone)]
pub struct RequestChannel {
    cid: u64,
    pub(crate) expected_requests: Waiters,
    pub(crate) bypass_ism_outbound_tx: Option<BypasserTx>,
}

impl RequestChannel {
    pub fn new(cid: u64, bypass: BypasserTx) -> Self {
        Self {
            cid,
            expected_requests: Arc::new(DashMap::new()),
            bypass_ism_outbound_tx: Some(bypass),
        }
    }

    /// The reply some request here is waiting for is consumed (`None`); any
    /// other response is handed back for the application.
    pub fn claim_reply(
        &self,
        response: InternalServiceResponse,
    ) -> Option<InternalServiceResponse> {
        if let Some(id) = response.request_id() {
            if let Some(tx) = self.expected_requests.remove(id) {
                let _ = tx.1.send(response);
                return None;
            }
        }
        Some(response)
    }

    /// Dropping the sender wakes the waiter with the connection-lost error.
    pub fn abandon(&self, request_id: &Uuid) {
        self.expected_requests.remove(request_id);
    }

    pub fn abandon_all(&self) {
        self.expected_requests.clear();
    }

    /// Send `request` and wait for the reply carrying `request_id`.
    ///
    /// The reply slot is registered BEFORE the request leaves. It used to be
    /// registered after, and the agent is local: a reply inspected in between
    /// matched nothing, was passed on as uncaught, and its waiter timed out five
    /// seconds later on an answer that had already arrived (see the
    /// `a_reply_that_beats_its_waiter` test).
    ///
    /// There is no deadline, on purpose. The agent is local, the connection is
    /// reliable and ordered, and the agent answers every LocalDB and Batched
    /// request it receives (a refusal is an answer). So an unanswered request
    /// means one thing — the connection is gone — and that is reported, at
    /// once, by the messenger calling [`CitadelBackendExt::abandon_all_requests`]
    /// when its connection ends. Everything else is lateness.
    ///
    /// It had a five-second deadline, and a stalled agent (a debug binary
    /// symbolising a backtrace, a rekey's keygen, a loaded machine) turned
    /// lateness into failure: the write was reported as "may not be stored"
    /// while it was in fact stored, and the reply that confirmed it matched no
    /// waiter and was passed on to the application as unsolicited. The browser
    /// runs this same code over its WebSocket, with the same deadline (wasmtimer
    /// on wasm32), so a slow agent failed browser sends exactly the same way.
    ///
    /// Cancellation-safe without a guard: if this future is dropped, the slot
    /// stays until its reply arrives, and `inspect_received_payload` still
    /// consumes that reply as the backend's own rather than forwarding it. The
    /// slot cannot outlive the connection, which is the only thing that could
    /// stop the reply from coming.
    pub async fn send_and_wait(
        &self,
        request: InternalServiceRequest,
        request_id: Uuid,
    ) -> Result<InternalServiceResponse, BackendError<WrappedMessage>> {
        let (tx, rx) = citadel_io::tokio::sync::oneshot::channel();
        match self.expected_requests.entry(request_id) {
            // Displacing a waiter would drop its sender, which now reads as
            // "connection lost", and hand this request's reply to nobody.
            Entry::Occupied(_) => {
                return Err(BackendError::StorageError(format!(
                    "request_id {request_id} is already awaiting a reply"
                )))
            }
            Entry::Vacant(slot) => {
                slot.insert(tx);
            }
        }
        if let Err(err) = self.send_to_network(request).await {
            // Nothing will ever answer a request that never left.
            self.expected_requests.remove(&request_id);
            return Err(err);
        }
        rx.await.map_err(|_| {
            BackendError::StorageError(format!(
                "The connection to the agent closed before it answered request_id {request_id} \
                 (CID {}); whether it was applied is unknown",
                self.cid
            ))
        })
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
}

#[async_trait]
impl BackendChannel for RequestChannel {
    async fn request(
        &self,
        request: InternalServiceRequest,
        request_id: Uuid,
    ) -> Result<InternalServiceResponse, BackendError<WrappedMessage>> {
        self.send_and_wait(request, request_id).await
    }
}

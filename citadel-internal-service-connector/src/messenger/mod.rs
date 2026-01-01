use crate::connector::InternalServiceConnector;
use crate::io_interface::IOInterface;
use crate::messenger::backend::CitadelBackendExt;
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use citadel_internal_service_types::{
    InternalServicePayload, InternalServiceRequest, InternalServiceResponse, SecurityLevel,
    SessionInformation,
};
use citadel_io::tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use citadel_io::tokio::sync::Mutex;
use citadel_logging as log;
use dashmap::DashMap;
use futures::future::Either;
use futures::{SinkExt, StreamExt};
use intersession_layer_messaging::{
    DeliveryError, MessageMetadata, NetworkError, Payload, UnderlyingSessionTransport, ILM,
};
use itertools::Itertools;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures;

pub mod backend;

/// Platform-agnostic async sleep function
/// - Native: Uses tokio::time::sleep
/// - WASM: Uses wasmtimer::tokio::sleep
#[cfg(not(target_arch = "wasm32"))]
pub async fn sleep_internal(duration: Duration) {
    citadel_io::tokio::time::sleep(duration).await;
}

#[cfg(target_arch = "wasm32")]
pub async fn sleep_internal(duration: Duration) {
    wasmtimer::tokio::sleep(duration).await;
}

/// Platform-agnostic async timeout function
/// - Native: Uses tokio::time::timeout
/// - WASM: Uses wasmtimer::tokio::timeout
#[cfg(not(target_arch = "wasm32"))]
pub async fn timeout_internal<F, T>(duration: Duration, future: F) -> Result<T, ()>
where
    F: Future<Output = T>,
{
    citadel_io::tokio::time::timeout(duration, future)
        .await
        .map_err(|_| ())
}

#[cfg(target_arch = "wasm32")]
pub async fn timeout_internal<F, T>(duration: Duration, future: F) -> Result<T, ()>
where
    F: Future<Output = T>,
{
    wasmtimer::tokio::timeout(duration, future)
        .await
        .map_err(|_| ())
}

/// A multiplexer for the InternalServiceConnector that allows for multiple handles
pub struct CitadelWorkspaceMessenger<B>
where
    B: CitadelBackendExt,
{
    // Local subscriptions where the key is the CID
    txs_to_inbound: Arc<DashMap<StreamKey, UnboundedSender<InternalMessage>>>,
    bypass_ism_tx_to_outbound: UnboundedSender<(StreamKey, InternalMessage)>,
    /// Periodically refreshed by the Messenger
    connected_peers: Arc<RwLock<HashMap<u64, SessionInformation>>>,
    // Contains a list of requests that were invoked by the background task and not to be delivered
    // to ISM or the end user
    background_invoked_requests: Arc<parking_lot::Mutex<HashSet<Uuid>>>,
    is_running: Arc<AtomicBool>,
    backends: Arc<DashMap<u64, B>>,
    final_tx: UnboundedSender<InternalServiceResponse>,
    /// Pending inbound messages for CIDs that don't have ILM handles yet.
    /// This fixes the race condition where messages arrive before multiplex() is called.
    pending_inbound_messages: Arc<DashMap<u64, Vec<InternalMessage>>>,
}

impl<B> Clone for CitadelWorkspaceMessenger<B>
where
    B: CitadelBackendExt,
{
    fn clone(&self) -> Self {
        Self {
            txs_to_inbound: self.txs_to_inbound.clone(),
            bypass_ism_tx_to_outbound: self.bypass_ism_tx_to_outbound.clone(),
            connected_peers: self.connected_peers.clone(),
            background_invoked_requests: self.background_invoked_requests.clone(),
            is_running: self.is_running.clone(),
            final_tx: self.final_tx.clone(),
            backends: self.backends.clone(),
            pending_inbound_messages: self.pending_inbound_messages.clone(),
        }
    }
}

pub type InternalMessage = Payload<WrappedMessage>;
const POLL_CONNECTED_PEERS_REFRESH_PERIOD: Duration = Duration::from_millis(1000);
const ISM_STREAM_ID: u8 = 0;
const BYPASS_ISM_STREAM_ID: u8 = 1;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct StreamKey {
    pub cid: u64,
    pub stream_id: u8,
}

impl StreamKey {
    const fn bypass_ism() -> Self {
        StreamKey {
            cid: LOOPBACK_ONLY,
            stream_id: BYPASS_ISM_STREAM_ID,
        }
    }
}

#[derive(Clone)]
pub struct BypasserTx {
    tx: UnboundedSender<(StreamKey, InternalMessage)>,
    stream_key: StreamKey,
}

impl BypasserTx {
    /// Sends an arbitrary request to the internal service. Not processed by the ISM layer.
    pub async fn send(
        &self,
        request: impl Into<InternalServicePayload>,
    ) -> Result<(), MessengerError> {
        let payload = Payload::Message(WrappedMessage {
            source_id: self.stream_key.cid,
            destination_id: LOOPBACK_ONLY,
            message_id: 0, // Does not matter since this will bypass ISM
            contents: request.into(),
        });

        let bypass_key = StreamKey::bypass_ism();

        self.tx.send((bypass_key, payload)).map_err(|err| {
            let reason = err.to_string();
            match err.0 .1 {
                Payload::Message(message) => MessengerError::SendError {
                    reason,
                    message: Either::Right(message.contents),
                },
                _ => MessengerError::OtherError { reason },
            }
        })
    }
}

type CitadelWorkspaceISM<B> = ILM<WrappedMessage, B, LocalDeliveryTx, ISMHandle<B>>;

pub struct LocalDeliveryTx {
    final_tx: UnboundedSender<InternalServiceResponse>,
}

#[async_trait]
impl intersession_layer_messaging::local_delivery::LocalDelivery<WrappedMessage>
    for LocalDeliveryTx
{
    async fn deliver(&self, message: WrappedMessage) -> Result<(), DeliveryError> {
        let InternalServicePayload::Response(response) = message.contents else {
            return Err(DeliveryError::BadInput);
        };

        self.final_tx
            .send(response)
            .map_err(|_| DeliveryError::ChannelClosed)
    }
}

#[derive(Serialize, Deserialize)]
pub enum WireWrapper {
    Message {
        source: u64,
        destination: u64,
        message_id: u64,
        contents: Vec<u8>,
    },
    ISMAux {
        signal: Box<InternalMessage>,
    },
}

impl<B> CitadelWorkspaceMessenger<B>
where
    B: CitadelBackendExt,
{
    pub fn new<T: IOInterface>(
        connector: InternalServiceConnector<T>,
    ) -> (Self, UnboundedReceiver<InternalServiceResponse>) {
        let (final_tx, final_rx) = citadel_io::tokio::sync::mpsc::unbounded_channel();
        // background layer
        let (bypass_ism_tx_to_outbound, rx_to_outbound) =
            citadel_io::tokio::sync::mpsc::unbounded_channel();
        let this = Self {
            backends: Arc::new(Default::default()),
            txs_to_inbound: Arc::new(Default::default()),
            connected_peers: Arc::new(Default::default()),
            is_running: Arc::new(AtomicBool::new(true)),
            background_invoked_requests: Arc::new(parking_lot::Mutex::new(Default::default())),
            bypass_ism_tx_to_outbound,
            final_tx,
            pending_inbound_messages: Arc::new(Default::default()),
        };

        this.spawn_background_tasks(connector, rx_to_outbound);

        (this, final_rx)
    }

    pub fn bypasser(&self) -> BypasserTx {
        BypasserTx {
            tx: self.bypass_ism_tx_to_outbound.clone(),
            stream_key: StreamKey::bypass_ism(),
        }
    }

    /// Multiplexes the messenger, creating a unique handle for the specified local client, `cid`
    pub async fn multiplex(&self, cid: u64) -> Result<MessengerTx<B>, MessengerError> {
        let stream_key = StreamKey {
            cid,
            stream_id: ISM_STREAM_ID,
        };
        log::info!(target: "ism", "[MULTIPLEX] Creating ILM handle for CID {} with stream_key={:?}", cid, stream_key);

        let (ism_handle, background_handle) = create_ipc_handles(self.clone(), stream_key);
        let local_delivery_wrapper = LocalDeliveryTx {
            final_tx: self.final_tx.clone(),
        };

        let BackgroundHandle {
            mut background_from_ism_outbound,
            background_to_ism_inbound,
        } = background_handle;
        self.txs_to_inbound
            .insert(stream_key, background_to_ism_inbound.clone());

        let all_keys: Vec<StreamKey> = self.txs_to_inbound.iter().map(|r| *r.key()).collect();
        log::info!(target: "ism", "[MULTIPLEX] Registered ILM for CID {}. All registered stream_keys: {:?}", cid, all_keys);

        // Process any pending messages that arrived before the ILM was ready.
        // This fixes the race condition where messages/ACKs arrive before multiplex() is called.
        if let Some((_, pending_messages)) = self.pending_inbound_messages.remove(&cid) {
            let count = pending_messages.len();
            log::info!(target: "ism", "[MULTIPLEX] Processing {} pending messages for CID {}", count, cid);
            for msg in pending_messages {
                if let Err(err) = background_to_ism_inbound.send(msg) {
                    log::error!(target: "ism", "[MULTIPLEX] Failed to deliver pending message: {err:?}");
                }
            }
            log::info!(target: "ism", "[MULTIPLEX] Delivered {} pending messages for CID {}", count, cid);
        }

        let mut handle = MessengerTx {
            bypass_ism_outbound_tx: BypasserTx {
                tx: self.bypass_ism_tx_to_outbound.clone(),
                stream_key,
            },
            messenger: self.clone(),
            stream_key,
            ism: None,
        };

        let backend =
            B::new(cid, &handle)
                .await
                .map_err(|err| MessengerError::CreateMultiplexError {
                    reason: format!("{err:?}"),
                })?;

        self.backends.insert(cid, backend.clone());

        let ism = ILM::new(backend, local_delivery_wrapper, ism_handle)
            .await
            .map_err(|err| MessengerError::OtherError {
                reason: format!("{err:?}"),
            })?;

        handle.ism = Some(ism);

        // We only have to spawn a single task: the one that listens for messages from the ISM and forwards them to the background task
        let this = self.clone();
        let task = async move {
            while let Some(message) = background_from_ism_outbound.recv().await {
                if !this.is_running() {
                    return;
                }

                if let Err(err) = this.bypass_ism_tx_to_outbound.send((stream_key, message)) {
                    log::error!(target: "citadel", "Error while sending message outbound: {err:?}");
                    break;
                }
            }
        };

        #[cfg(not(target_arch = "wasm32"))]
        drop(citadel_io::tokio::task::spawn(task));

        #[cfg(target_arch = "wasm32")]
        drop(wasm_bindgen_futures::spawn_local(task));

        Ok(handle)
    }

    fn spawn_background_tasks<T: IOInterface>(
        &self,
        connector: InternalServiceConnector<T>,
        mut rx_to_outbound: UnboundedReceiver<(StreamKey, InternalMessage)>,
    ) {
        let InternalServiceConnector::<T> {
            mut sink,
            mut stream,
        } = connector;

        let bypass_key = StreamKey::bypass_ism();

        let this = self.clone();
        let tx_to_local_user_clone = self.final_tx.clone();
        let network_inbound_task = async move {
            while let Some(network_message) = stream.next().await {
                if !this.is_running() {
                    return;
                }

                log::trace!(target: "citadel", "Received message from network layer: {network_message:?}");
                match network_message {
                    // TODO: Add support for group messaging
                    InternalServiceResponse::MessageNotification(mut message) => {
                        // deserialize and relay to ISM
                        match bincode2::deserialize::<WireWrapper>(&message.message) {
                            Ok(ism_message) => {
                                // ISM message processing

                                let ism_message = match ism_message {
                                    WireWrapper::ISMAux { signal } => *signal,
                                    WireWrapper::Message {
                                        contents,
                                        source,
                                        destination,
                                        message_id,
                                    } => {
                                        // Replace message bytes with unwrapped content
                                        let _ = std::mem::replace(
                                            &mut message.message,
                                            BytesMut::from(Bytes::from(contents)),
                                        );

                                        // CRITICAL FIX: Forward UNWRAPPED MessageNotification to JavaScript.
                                        // This ensures the frontend receives messages immediately with:
                                        // 1. Correct peer_cid (from original MessageNotification)
                                        // 2. Unwrapped message bytes (deserializable as P2PCommand)
                                        //
                                        // Previously, ISM messages were ONLY routed to ISM, which caused
                                        // the leader tab to never receive P2P messages because ISM
                                        // delivery wasn't working correctly in multi-tab scenarios.
                                        let forward_message = InternalServiceResponse::MessageNotification(
                                            message.clone()
                                        );
                                        if let Err(err) = tx_to_local_user_clone.send(forward_message) {
                                            log::error!(target: "citadel", "Error forwarding ISM MessageNotification to JS: {err:?}");
                                        } else {
                                            log::trace!(target: "citadel", "Forwarded ISM MessageNotification to JS (cid={}, peer_cid={})",
                                                message.cid, message.peer_cid);
                                        }

                                        InternalMessage::Message(WrappedMessage {
                                            source_id: source,
                                            destination_id: destination,
                                            message_id,
                                            contents: InternalServicePayload::Response(
                                                InternalServiceResponse::MessageNotification(
                                                    message,
                                                ),
                                            ),
                                        })
                                    }
                                };

                                let stream_key = StreamKey {
                                    cid: ism_message.destination_id(),
                                    stream_id: ISM_STREAM_ID,
                                };

                                let available_keys: Vec<StreamKey> = this.txs_to_inbound.iter().map(|r| *r.key()).collect();
                                log::info!(target: "ism", "[MSG-ROUTE] Routing message: source={} dest={} msg_id={} | Looking for stream_key={:?} | Available: {:?}",
                                    ism_message.source_id(), ism_message.destination_id(),
                                    match &ism_message { InternalMessage::Message(m) => m.message_id, _ => 0 },
                                    stream_key, available_keys);

                                if let Some(tx) = this.txs_to_inbound.get(&stream_key) {
                                    if let Err(err) = tx.send(ism_message) {
                                        log::error!(target: "citadel", "[MSG-ROUTE] FAILED to send to ISM channel: {err:?}");
                                    } else {
                                        log::info!(target: "ism", "[MSG-ROUTE] SUCCESS - sent to ILM for dest={}", stream_key.cid);
                                    }
                                } else {
                                    // Queue message for later delivery when multiplex() is called.
                                    // This fixes the race condition where messages arrive before ILM is ready.
                                    log::warn!(target: "ism", "[MSG-ROUTE] QUEUED - No ILM registered for CID {}. Queuing for later delivery. Available: {:?}",
                                        stream_key.cid, available_keys);
                                    this.pending_inbound_messages
                                        .entry(stream_key.cid)
                                        .or_default()
                                        .push(ism_message);
                                }
                            }
                            Err(err) => {
                                log::warn!(target: "citadel", "Error while deserializing ISM (?) message: {err:?}");
                                // Forward as is. Likely sent by a non-ISM peer
                                // Send to default direct handle
                                // TODO: Consider having the bypasser send directly to underlying stream
                                let other_message_type =
                                    InternalServiceResponse::MessageNotification(message);
                                if let Err(err) = tx_to_local_user_clone.send(other_message_type) {
                                    log::error!(target: "citadel", "Error while sending message to local user: {err:?}");
                                    return;
                                }
                            }
                        }
                    }

                    non_ism_message => {
                        // Process all other messages here
                        if matches!(
                            non_ism_message,
                            InternalServiceResponse::MessageSendSuccess(..)
                        ) {
                            continue;
                        }
                        // In the special case of GetSessionsResponse, we update the state
                        if let InternalServiceResponse::GetSessionsResponse(sessions) =
                            &non_ism_message
                        {
                            let mut lock = this.connected_peers.write();
                            let previous_local_cids =
                                lock.keys().copied().sorted().collect::<Vec<_>>();
                            let current_local_cid = sessions
                                .sessions
                                .iter()
                                .map(|r| r.cid)
                                .sorted()
                                .collect::<Vec<_>>();

                            // Cleanup any stale sessions
                            for previous_local_cid in previous_local_cids {
                                if !current_local_cid.contains(&previous_local_cid) {
                                    lock.remove(&previous_local_cid);
                                }
                            }

                            for session in &sessions.sessions {
                                let cid = session.cid;
                                let value = session.clone();
                                lock.insert(cid, value);
                            }
                        }

                        if let Some(request_id) = non_ism_message.request_id() {
                            if this.background_invoked_requests.lock().remove(request_id) {
                                log::trace!(target: "citadel", "[BYPASS] Inbound message will not be forwarded to ISM: {non_ism_message:?}");
                                // This was a request invoked by the background task. Do not deliver to ISM
                                continue;
                            }
                        }

                        // Pass through backend inspection
                        let uncaught_non_ism_message =
                            if let Some(backend) = this.backends.get(&non_ism_message.cid()) {
                                // Found backend by CID - use it
                                let backend = backend.value();
                                if let Some(value) = backend
                                    .inspect_received_payload(non_ism_message)
                                    .await
                                    .ok()
                                    .flatten()
                                {
                                    value
                                } else {
                                    continue;
                                }
                            } else if non_ism_message.request_id().is_some() {
                                // No backend for CID - check ALL backends for this request_id
                                // This handles BatchedResponse and other CID-0 responses
                                let mut consumed = false;
                                for entry in this.backends.iter() {
                                    let backend = entry.value();
                                    if backend
                                        .inspect_received_payload(non_ism_message.clone())
                                        .await
                                        .ok()
                                        .flatten()
                                        .is_none()
                                    {
                                        // Backend consumed the message (it was waiting for this request_id)
                                        consumed = true;
                                        break;
                                    }
                                }
                                if consumed {
                                    continue;
                                }
                                non_ism_message
                            } else {
                                non_ism_message
                            };

                        // Send to default direct handle
                        if let Err(err) = tx_to_local_user_clone.send(uncaught_non_ism_message) {
                            log::error!(target: "citadel", "Error while sending message to local user: {err:?}");
                            return;
                        }
                    }
                }
            }
        };

        let this = self.clone();
        // Takes messages sent through ISM or a direct request and funnels them here
        let ism_to_background_task_outbound = async move {
            while let Some((stream_key, message_internal)) = rx_to_outbound.recv().await {
                log::trace!(target: "citadel", "Received message from ISM or background: {stream_key:?}: {message_internal:?}");
                if !this.is_running() {
                    return;
                }

                // We get one of two types of messages here:
                // 1) Handle -> ISM -> Here, in which case, it is for messaging between nodes, or;
                // 2) Handle -> Here, in which case, it is a request for the internal service
                match message_internal {
                    Payload::Message(message) if stream_key == bypass_key => {
                        // This is a message for the internal service
                        let InternalServicePayload::Request(request) = message.contents else {
                            log::warn!(target: "citadel", "Received a message with no destination that was not a request: {message:?}");
                            continue;
                        };

                        log::trace!(target: "citadel", "[Bypass] Received a message for the internal service: {request:?}");

                        if let Err(err) = sink.send(request).await {
                            log::error!(target: "citadel", "Error while sending ISM message to outbound network: {err:?}")
                        }
                    }

                    Payload::Message(mut ism_message) if stream_key != bypass_key => {
                        let InternalServicePayload::Request(InternalServiceRequest::Message {
                            request_id,
                            message,
                            cid,
                            peer_cid,
                            security_level,
                        }) = &mut ism_message.contents
                        else {
                            log::warn!(target: "citadel", "Received an outgoing request for another node that was not a message: {stream_key:?}");
                            continue;
                        };

                        // Create a new request where the payload is the serialized WireWrapper
                        // The contents of the wirewrapper take the original message and the source and destination for preservation
                        let wire_message = WireWrapper::Message {
                            contents: std::mem::take(message),
                            source: ism_message.source_id,
                            destination: ism_message.destination_id,
                            message_id: ism_message.message_id,
                        };

                        let request = InternalServiceRequest::Message {
                            request_id: *request_id,
                            message: bincode2::serialize(&wire_message)
                                .expect("Should be able to serialize message"),
                            cid: *cid,
                            peer_cid: *peer_cid,
                            security_level: *security_level,
                        };

                        if let Err(err) = sink.send(request).await {
                            log::error!(target: "citadel", "Error while sending ISM message to outbound network: {err:?}");
                            return;
                        }
                    }

                    ism_proto if stream_key != bypass_key => {
                        // This is an ACK/POLL message which needs to be manually wrapped into a message
                        log::info!(target: "citadel", "[NO-BYPASS] Received a message for another node: {stream_key:?}");
                        let cid = ism_proto.source_id();
                        let peer_cid = ism_proto.destination_id();
                        let wire_message = WireWrapper::ISMAux {
                            signal: Box::new(ism_proto),
                        };
                        let serialized_message = bincode2::serialize(&wire_message)
                            .expect("Should be able to serialize message");
                        // TODO: Add support for group messaging
                        let message_request = InternalServiceRequest::Message {
                            request_id: Uuid::new_v4(),
                            message: serialized_message,
                            cid,
                            peer_cid: Some(peer_cid),
                            security_level: Default::default(),
                        };

                        if let Err(err) = sink.send(message_request).await {
                            log::error!(target: "citadel", "Error while sending ISM message to outbound network: {err:?}");
                            return;
                        }
                    }

                    other => {
                        log::warn!(target: "citadel", "Received a message for an invalid stream {stream_key:?}: {other:?}");
                    }
                }
            }
        };

        let periodic_session_status_poller = {
            let this = self.clone();
            async move {
                loop {
                    if !this.is_running() {
                        return;
                    }

                    sleep_internal(POLL_CONNECTED_PEERS_REFRESH_PERIOD).await;
                    let mut background_invoked_requests = this.background_invoked_requests.lock();
                    // Update the state for each client that has a running ISM instance
                    let local_cids: Vec<u64> =
                        this.txs_to_inbound.iter().map(|r| r.key().cid).collect();
                    for source_cid in local_cids {
                        let request_id = Uuid::new_v4();
                        background_invoked_requests.insert(request_id);
                        // The inbound network task will automatically update the state. All this task has to do
                        // is send the request
                        let request = InternalMessage::Message(WrappedMessage {
                            source_id: source_cid,
                            destination_id: LOOPBACK_ONLY,
                            message_id: 0,
                            contents: InternalServicePayload::Request(
                                InternalServiceRequest::GetSessions { request_id },
                            ),
                        });

                        log::trace!(target: "citadel", "[POLL] Sending session status poller request {bypass_key:?}: {request:?}");
                        if let Err(err) = this.bypass_ism_tx_to_outbound.send((bypass_key, request))
                        {
                            log::error!(target: "citadel", "Error while sending session status poller request: {err:?}");
                            return;
                        }
                    }
                }
            }
        };

        let this = self.clone();
        let task = async move {
            citadel_io::tokio::select! {
                _ = network_inbound_task => {
                    log::error!(target: "citadel", "Network inbound task ended. Messenger is shutting down")
                },

                _ = ism_to_background_task_outbound => {
                    log::error!(target: "citadel", "Local handle to ISM outbound task ended. Messenger is shutting down")
                }

                _ = periodic_session_status_poller => {
                    log::error!(target: "citadel", "Periodic session status poller ended. Messenger is shutting down")
                }
            }

            this.is_running.store(false, Ordering::SeqCst);
        };

        #[cfg(not(target_arch = "wasm32"))]
        drop(citadel_io::tokio::task::spawn(task));

        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(task);
    }

    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }
}

pub struct MessengerTx<B>
where
    B: CitadelBackendExt,
{
    bypass_ism_outbound_tx: BypasserTx,
    messenger: CitadelWorkspaceMessenger<B>,
    stream_key: StreamKey,
    ism: Option<CitadelWorkspaceISM<B>>,
}

impl<B> Drop for MessengerTx<B>
where
    B: CitadelBackendExt,
{
    fn drop(&mut self) {
        // Remove the handle from the list of active handles
        self.messenger.txs_to_inbound.remove(&self.stream_key);
    }
}

pub fn send_from_non_ism_source_to_ism_destination(
    cid: u64,
    payload: impl Into<InternalServicePayload>,
) -> WireWrapper {
    // In order for the receiver to decode the response, we must make the response ISM-compatible
    WireWrapper::ISMAux {
        signal: Box::new(InternalMessage::Message(WrappedMessage {
            source_id: cid,
            destination_id: LOOPBACK_ONLY,
            message_id: 0, // Does not matter since this isn't tracked
            contents: payload.into(),
        })),
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WrappedMessage {
    source_id: u64,
    destination_id: u64,
    message_id: u64,
    contents: InternalServicePayload,
}

/// When a message is sent for purposes of polling the internal service
const LOOPBACK_ONLY: u64 = 0;

impl MessageMetadata for WrappedMessage {
    type PeerId = u64;
    type MessageId = u64;
    type Contents = InternalServicePayload;

    fn source_id(&self) -> Self::PeerId {
        self.source_id
    }

    fn destination_id(&self) -> Self::PeerId {
        self.destination_id
    }

    fn message_id(&self) -> Self::MessageId {
        self.message_id
    }

    fn contents(&self) -> &Self::Contents {
        &self.contents
    }

    fn construct_from_parts(
        source_id: Self::PeerId,
        destination_id: Self::PeerId,
        message_id: Self::MessageId,
        contents: impl Into<Self::Contents>,
    ) -> Self {
        Self {
            source_id,
            destination_id,
            message_id,
            contents: contents.into(),
        }
    }
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum MessengerError {
    SendError {
        reason: String,
        message: Either<Vec<u8>, InternalServicePayload>,
    },
    OtherError {
        reason: String,
    },
    InitError {
        reason: String,
    },
    CreateMultiplexError {
        reason: String,
    },
    Shutdown,
}

impl Display for MessengerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}

impl std::error::Error for MessengerError {}

impl<B> MessengerTx<B>
where
    B: CitadelBackendExt,
{
    /// Waits for a peer to connect to the local client
    pub async fn wait_for_peer_to_connect(&self, peer_cid: u64) -> Result<(), MessengerError> {
        let interval = Duration::from_millis(300);
        loop {
            if !self.messenger.is_running() {
                return Err(MessengerError::Shutdown);
            }

            sleep_internal(interval).await;

            if self.get_connected_peers().await.contains(&peer_cid) {
                return Ok(());
            }
        }
    }

    /// Returns a list of peers that the local client is connected to
    pub async fn get_connected_peers(&self) -> Vec<u64> {
        self.get_ism().get_connected_peers().await
    }

    pub fn local_cid(&self) -> u64 {
        self.stream_key.cid
    }

    pub async fn send_message_to(
        &self,
        peer_cid: u64,
        message: impl Into<Vec<u8>>,
    ) -> Result<(), MessengerError> {
        self.send_message_to_with_security_level(peer_cid, Default::default(), message)
            .await
    }

    pub async fn send_message_to_with_security_level(
        &self,
        peer_cid: u64,
        security_level: SecurityLevel,
        message: impl Into<Vec<u8>>,
    ) -> Result<(), MessengerError> {
        self.send_message_to_with_security_level_and_req_id(
            peer_cid,
            security_level,
            Uuid::new_v4(),
            message,
        )
        .await
    }

    pub async fn send_message_to_with_security_level_and_req_id(
        &self,
        peer_cid: u64,
        security_level: SecurityLevel,
        request_id: Uuid,
        message: impl Into<Vec<u8>>,
    ) -> Result<(), MessengerError> {
        let payload = InternalServicePayload::Request(InternalServiceRequest::Message {
            request_id,
            message: message.into(),
            cid: self.stream_key.cid,
            peer_cid: Some(peer_cid),
            security_level,
        });

        // Send to ISM layer. ISM will then send to the background task using the sink in the UnderlyingNetworkTransport impl
        self.send_message_to_ism(peer_cid, payload).await
    }

    /// Sends an arbitrary request to the internal service. Not processed by the ISM layer.
    pub async fn send_request(
        &self,
        request: impl Into<InternalServicePayload>,
    ) -> Result<(), MessengerError> {
        self.bypass_ism_outbound_tx.send(request).await
    }

    async fn send_message_to_ism(
        &self,
        peer_cid: u64,
        request: impl Into<InternalServicePayload>,
    ) -> Result<(), MessengerError> {
        self.get_ism()
            .send_to(peer_cid, request.into())
            .await
            .map_err(|err| match err {
                NetworkError::SendFailed { reason, message } => match message.contents {
                    InternalServicePayload::Request(InternalServiceRequest::Message {
                        message,
                        ..
                    }) => MessengerError::SendError {
                        reason,
                        message: Either::Left(message),
                    },
                    other => MessengerError::SendError {
                        reason,
                        message: Either::Right(other),
                    },
                },
                err => MessengerError::OtherError {
                    reason: format!("{err:?}"),
                },
            })
    }

    fn get_ism(&self) -> &CitadelWorkspaceISM<B> {
        self.ism.as_ref().expect("ISM should be initialized")
    }
}

/// This implements UnderlyingSessionTransport. It is responsible for sending messages from ISM
/// to the background outbound task. It is also responsible for receiving messages from the background task
/// and forwarding them to the ISM for processing.
struct ISMHandle<B>
where
    B: CitadelBackendExt,
{
    ism_to_background_outbound: UnboundedSender<InternalMessage>,
    ism_from_background_inbound: Mutex<UnboundedReceiver<InternalMessage>>,
    messenger_ptr: CitadelWorkspaceMessenger<B>,
    stream_key: StreamKey,
}

/// This is what the background will use to interact with the ISM
struct BackgroundHandle {
    background_from_ism_outbound: UnboundedReceiver<InternalMessage>,
    background_to_ism_inbound: UnboundedSender<InternalMessage>,
}

fn create_ipc_handles<B>(
    messenger_ptr: CitadelWorkspaceMessenger<B>,
    stream_key: StreamKey,
) -> (ISMHandle<B>, BackgroundHandle)
where
    B: CitadelBackendExt,
{
    let (ism_to_background_outbound, background_from_ism_outbound) =
        citadel_io::tokio::sync::mpsc::unbounded_channel();
    let (background_to_ism_inbound, ism_from_background_inbound) =
        citadel_io::tokio::sync::mpsc::unbounded_channel();

    (
        ISMHandle {
            ism_to_background_outbound,
            ism_from_background_inbound: Mutex::new(ism_from_background_inbound),
            messenger_ptr,
            stream_key,
        },
        BackgroundHandle {
            background_from_ism_outbound,
            background_to_ism_inbound,
        },
    )
}

#[async_trait]
impl<B> UnderlyingSessionTransport for ISMHandle<B>
where
    B: CitadelBackendExt,
{
    type Message = WrappedMessage;

    async fn next_message(&self) -> Option<Payload<Self::Message>> {
        self.ism_from_background_inbound
            .try_lock()
            .expect("There should be only one caller polling for messages")
            .recv()
            .await
    }

    async fn send_message(
        &self,
        message: Payload<Self::Message>,
    ) -> Result<(), NetworkError<Payload<Self::Message>>> {
        self.ism_to_background_outbound
            .send(message)
            .map_err(|err| NetworkError::SendFailed {
                reason: err.to_string(),
                message: err.0,
            })
    }

    /// Returns the list of connected peer CIDs for this session.
    ///
    /// For WASM target: Calls JavaScript callback `window.__citadel_get_peers_for_session(local_cid)`
    /// which queries the TypeScript P2PAutoConnectService (single source of truth).
    ///
    /// For native target: Uses internal connected_peers map from GetSessionsResponse polling.
    #[cfg(target_arch = "wasm32")]
    async fn connected_peers(&self) -> Vec<<Self::Message as MessageMetadata>::PeerId> {
        use wasm_bindgen::prelude::*;

        #[wasm_bindgen]
        extern "C" {
            #[wasm_bindgen(js_namespace = window, js_name = __citadel_get_peers_for_session)]
            fn get_peers_for_session(local_cid: u64) -> js_sys::BigUint64Array;
        }

        let cid = self.local_id();
        let js_array = get_peers_for_session(cid);

        // Convert BigUint64Array to Vec<u64>
        let length = js_array.length() as usize;
        let mut result = Vec::with_capacity(length);
        for i in 0..length {
            result.push(js_array.get_index(i as u32));
        }

        log::trace!(target: "citadel", "[WASM] connected_peers({cid}) -> {} peers from TypeScript", result.len());
        result
    }

    /// Native implementation: Uses internal connected_peers map from GetSessionsResponse polling.
    #[cfg(not(target_arch = "wasm32"))]
    async fn connected_peers(&self) -> Vec<<Self::Message as MessageMetadata>::PeerId> {
        let cid = self.local_id();
        if let Some(sess) = self.messenger_ptr.connected_peers.read().get(&cid) {
            sess.peer_connections.values().map(|r| r.peer_cid).collect()
        } else {
            log::warn!(target: "citadel", "No session information found for {cid}");
            vec![]
        }
    }

    fn local_id(&self) -> <Self::Message as MessageMetadata>::PeerId {
        self.stream_key.cid
    }
}

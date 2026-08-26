use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use citadel_internal_service_connector::connector::InternalServiceConnector;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_connector::messenger::{CitadelWorkspaceMessenger, MessengerTx};
use citadel_internal_service_types::{
    InternalServicePayload, InternalServiceRequest, InternalServiceResponse, SecurityLevel,
};

use async_trait::async_trait;
use citadel_io::tokio::sync::{RwLock, mpsc};
use dashmap::{DashMap, DashSet};
use futures::{Sink, Stream};
// use send_wrapper::SendWrapper;  // Not needed with channel-based approach
use ws_stream_wasm::{WsMessage, WsMeta};
// use futures_util::{SinkExt as FuturesSinkExt, StreamExt as FuturesStreamExt};
use once_cell::sync::{Lazy, OnceCell};
use std::sync::RwLock as StdRwLock;
use wasm_bindgen::prelude::*;

// WASM exports and logging setup - defined early for use in functions below
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);

    // Custom debug log function that will be provided by JavaScript
    // This will use the formatForDebug function on the JS side
    #[wasm_bindgen(js_name = wasmDebugLog)]
    fn debug_log(s: &str);

    // Callback to notify JavaScript when WebSocket connection dies
    // This allows the UI to show a retry modal
    #[wasm_bindgen(js_name = onWasmWebSocketDisconnected)]
    fn on_websocket_disconnected(reason: &str);
}

// For debugging objects that need formatting
// Allow unused - kept for debugging purposes, can be enabled by uncommenting calls
#[allow(unused_macros)]
macro_rules! debug_log {
    ($($t:tt)*) => (debug_log(&format_args!($($t)*).to_string()))
}

// For simple console logs that don't need formatting
macro_rules! console_log {
    ($($t:tt)*) => (log(&format_args!($($t)*).to_string()))
}

// Native BigInt serialization using serde-wasm-bindgen
// u64 values are converted to JavaScript BigInt to avoid overflow
fn serialize_response(response: &InternalServiceResponse) -> Result<JsValue, JsValue> {
    use serde::Serialize;

    // DEBUG: Log ListRegisteredPeersResponse before serialization to trace HashMap data loss
    if let InternalServiceResponse::ListRegisteredPeersResponse(resp) = response {
        console_log!(
            "[WASM-DEBUG] ListRegisteredPeersResponse BEFORE serialization: cid={}, peers.len()={}, peer_cids={:?}",
            resp.cid,
            resp.peers.len(),
            resp.peers.keys().collect::<Vec<_>>()
        );
    }

    let serializer =
        serde_wasm_bindgen::Serializer::new().serialize_large_number_types_as_bigints(true);
    response
        .serialize(&serializer)
        .map_err(|e| JsValue::from_str(&format!("Serialization error: {}", e)))
}

// Native BigInt deserialization using serde-wasm-bindgen
// JavaScript BigInt values are automatically converted to u64
fn deserialize_request(message: JsValue) -> Result<InternalServiceRequest, JsValue> {
    serde_wasm_bindgen::from_value(message)
        .map_err(|e| JsValue::from_str(&format!("Deserialization error: {}", e)))
}
// use std::time::Duration;

// Error types
#[derive(thiserror::Error, Debug)]
pub enum WasmClientError {
    #[error("WebSocket error: {0}")]
    WebSocket(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Connection not initialized")]
    NotInitialized,
    #[error("Connection error: {0}")]
    Connection(String),
    #[error("Channel error: {0}")]
    Channel(String),
}

// Channel-based WebSocket wrapper that implements the required Sink/Stream traits
pub struct WasmWebSocketSink {
    tx: mpsc::UnboundedSender<InternalServicePayload>,
}

pub struct WasmWebSocketStream {
    rx: mpsc::UnboundedReceiver<std::io::Result<InternalServicePayload>>,
}

impl Sink<InternalServicePayload> for WasmWebSocketSink {
    type Error = std::io::Error;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: InternalServicePayload) -> Result<(), Self::Error> {
        self.tx
            .send(item)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

impl Stream for WasmWebSocketStream {
    type Item = std::io::Result<InternalServicePayload>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

// Helper traits for Unpin
impl Unpin for WasmWebSocketSink {}
impl Unpin for WasmWebSocketStream {}

// Channel-based IO implementation for WASM
pub struct WasmWebSocketIO {
    sink_tx: Option<mpsc::UnboundedSender<InternalServicePayload>>,
    stream_rx: Option<mpsc::UnboundedReceiver<std::io::Result<InternalServicePayload>>>,
}

impl WasmWebSocketIO {
    pub fn new(
        sink_tx: mpsc::UnboundedSender<InternalServicePayload>,
        stream_rx: mpsc::UnboundedReceiver<std::io::Result<InternalServicePayload>>,
    ) -> Self {
        Self {
            sink_tx: Some(sink_tx),
            stream_rx: Some(stream_rx),
        }
    }
}

#[async_trait]
impl IOInterface for WasmWebSocketIO {
    type Sink = WasmWebSocketSink;
    type Stream = WasmWebSocketStream;

    async fn next_connection(&mut self) -> Option<(Self::Sink, Self::Stream)> {
        if let (Some(sink_tx), Some(stream_rx)) = (self.sink_tx.take(), self.stream_rx.take()) {
            let sink = WasmWebSocketSink { tx: sink_tx };
            let stream = WasmWebSocketStream { rx: stream_rx };
            Some((sink, stream))
        } else {
            console_log!("Already called next_connection");
            None
        }
    }
}

// Use the backend from the connector crate
use citadel_internal_service_connector::messenger::backend::CitadelWorkspaceBackend;

// Global state management
static WORKSPACE_STATE: OnceCell<Arc<RwLock<Option<WorkspaceState>>>> = OnceCell::new();
/// The WebSocket sink, replaced on every (re)connect.
///
/// Was a `OnceCell`, which made reconnection impossible AND destructive:
/// `restart()` calls `close_connection()` first and then re-runs the connect
/// path, where `OnceCell::set` returns `Err` because the cell was already
/// populated by the original `init()`. So every restart tore down a working
/// client and then failed with "Failed to store sink channel", leaving nothing
/// running — and every subsequent attempt failed the same way. The recovery
/// path in InternalServiceWasmClient therefore could never succeed; it retried
/// until it gave up.
///
/// A re-settable holder is the whole fix: connect writes, teardown clears.
static SINK_CHANNEL: Lazy<StdRwLock<Option<mpsc::UnboundedSender<InternalServicePayload>>>> =
    Lazy::new(|| StdRwLock::new(None));

/// The current sink, if a connection is up.
fn sink_channel() -> Option<mpsc::UnboundedSender<InternalServicePayload>> {
    SINK_CHANNEL.read().ok().and_then(|guard| guard.clone())
}

struct WorkspaceState {
    messenger: CitadelWorkspaceMessenger<CitadelWorkspaceBackend>,
    stream: Option<citadel_io::tokio::sync::mpsc::UnboundedReceiver<InternalServiceResponse>>,
    // Messages rescued from a stream that was replaced while `next_message` had
    // it checked out. Without this they were dropped with the receiver.
    pending: std::collections::VecDeque<InternalServiceResponse>,
    // Wrapped in Arc so we can clone handles and release the RwLock before long async operations
    connections: Arc<DashMap<u64, Arc<MessengerTx<CitadelWorkspaceBackend>>>>,
    // Track CIDs that are currently being opened to prevent duplicate multiplex calls
    pending_opens: Arc<DashSet<u64>>,
    // On drop, this will kill the background task automatically (RAII pattern)
    #[allow(dead_code)]
    shutdown_tx: citadel_io::tokio::sync::oneshot::Sender<()>,
}

// Initialize the global state
fn get_workspace_state() -> &'static Arc<RwLock<Option<WorkspaceState>>> {
    WORKSPACE_STATE.get_or_init(|| Arc::new(RwLock::new(None)))
}

// Remove duplicate - already defined above

#[wasm_bindgen(start)]
pub fn main() {
    // Set up a custom panic hook that:
    // 1. Logs the panic to the console (via console_error_panic_hook)
    // 2. Notifies JavaScript so the UI can show a retry modal
    std::panic::set_hook(Box::new(|panic_info| {
        // First, use the standard console error panic hook for logging
        console_error_panic_hook::hook(panic_info);

        // Then notify JavaScript about the panic so UI can show retry modal
        let panic_message = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            format!("WASM panic: {}", s)
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            format!("WASM panic: {}", s)
        } else {
            "WASM panic: Unknown error".to_string()
        };

        // Add location info if available
        let message = if let Some(location) = panic_info.location() {
            format!(
                "{} at {}:{}",
                panic_message,
                location.file(),
                location.line()
            )
        } else {
            panic_message
        };

        console_log!("[WASM] Panic detected, notifying JavaScript: {}", message);
        on_websocket_disconnected(&message);
    }));
}

#[wasm_bindgen]
pub async fn init(ws_url: String) -> Result<(), JsValue> {
    init_inner(ws_url, false).await
}

// Should only be called when manually triggered by the UI "Retry Now" button
#[wasm_bindgen]
pub async fn restart(ws_url: String) -> Result<(), JsValue> {
    init_inner(ws_url, true).await
}

async fn init_inner(ws_url: String, restart: bool) -> Result<(), JsValue> {
    if restart {
        if !is_initialized() {
            return Err(JsValue::from_str("Not initialized. Call init() first."));
        }
        console_log!("Restarting WASM client with URL: {}", ws_url);
        close_connection().await?;
    } else {
        if is_initialized() {
            return Err(JsValue::from_str(
                "Already initialized. If required, call restart() instead followed by claiming any orphaned connections.",
            ));
        }
        // Initialize the Rust log crate to route to browser console
        // This makes log::info!, log::warn!, etc. visible in DevTools
        console_log::init_with_level(log::Level::Info).ok();
        console_log!("Initializing WASM client with URL: {}", ws_url);
    }

    // Create channels for WebSocket communication
    let (sink_tx, mut sink_rx) = mpsc::unbounded_channel::<InternalServicePayload>();
    let (shutdown_tx, mut shutdown_rx) = citadel_io::tokio::sync::oneshot::channel::<()>();
    let (stream_tx, stream_rx) =
        mpsc::unbounded_channel::<std::io::Result<InternalServicePayload>>();

    // Store the sink channel globally for direct message sending
    // Replace, not set-once: a restart must be able to install a new sink.
    match SINK_CHANNEL.write() {
        Ok(mut guard) => *guard = Some(sink_tx.clone()),
        Err(_) => return Err(JsValue::from_str("Sink channel lock poisoned")),
    }

    // Connect to WebSocket and spawn communication task
    let (_ws_meta, ws_stream) = WsMeta::connect(&ws_url, None)
        .await
        .map_err(|e| JsValue::from_str(&format!("WebSocket connection failed: {:?}", e)))?;

    console_log!("WebSocket connected successfully");

    // Spawn task to handle WebSocket communication
    wasm_bindgen_futures::spawn_local(async move {
        use futures::{SinkExt, StreamExt};

        let (mut ws_sink, mut ws_stream) = ws_stream.split();

        loop {
            citadel_io::tokio::select! {
                _terminated = &mut shutdown_rx => {
                    console_log!("Shutdown signal received");
                    break;
                }
                // Handle outgoing messages from sink_rx
                Some(payload) = sink_rx.recv() => {
                    match serde_json::to_string(&payload) {
                        Ok(json) => {
                            //debug_log!("Sending JSON to server: {}", json);
                            let message = WsMessage::Text(json);
                            if let Err(e) = ws_sink.send(message).await {
                                console_log!("Error sending WebSocket message: {:?}", e);
                                break;
                            }
                            // Success case: No log needed - reduces console noise significantly
                            // (removed verbose "JSON message sent successfully" log)
                        }
                        Err(e) => {
                            console_log!("Error serializing payload: {:?}", e);
                            break;
                        }
                    }
                }

                // Handle incoming messages from WebSocket
                Some(message) = ws_stream.next() => {
                    match message {
                        WsMessage::Text(text) => {
                            // DEBUG: Log raw JSON for ListRegisteredPeersResponse to trace HashMap data
                            if text.contains("ListRegisteredPeersResponse") {
                                console_log!("[WASM-DEBUG] Raw JSON (ListRegisteredPeersResponse): {}", &text[..text.len().min(500)]);
                            }

                            match serde_json::from_str::<InternalServicePayload>(&text) {
                                Ok(payload) => {
                                    if stream_tx.send(Ok(payload)).is_err() {
                                        console_log!("Stream receiver closed");
                                        break;
                                    }
                                }
                                Err(e) => {
                                    console_log!("Error deserializing payload: {:?}", e);
                                    let err = std::io::Error::new(std::io::ErrorKind::InvalidData, e);
                                    if stream_tx.send(Err(err)).is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                        WsMessage::Binary(_) => {
                            console_log!("Received binary message (not supported)");
                        }
                    }
                }

                else => {
                    console_log!("WebSocket streams closed");
                    break;
                }
            }
        }

        console_log!("WebSocket communication task ended");

        // Notify JavaScript that the WebSocket connection has died
        // This allows the UI to show a retry modal
        on_websocket_disconnected("WebSocket communication task ended");
    });

    // Create IO implementation with channels
    let io_implementation = WasmWebSocketIO::new(sink_tx, stream_rx);

    let connector = InternalServiceConnector::from_io(io_implementation)
        .await
        .ok_or_else(|| JsValue::from_str("Failed to create connector"))?;

    let (messenger, stream) = CitadelWorkspaceMessenger::new(connector);
    let connections = Arc::new(DashMap::new());
    let pending_opens = Arc::new(DashSet::new());

    let state = WorkspaceState {
        messenger,
        stream: Some(stream),
        pending: std::collections::VecDeque::new(),
        connections,
        pending_opens,
        shutdown_tx,
    };

    let workspace_state = get_workspace_state();
    let mut guard = workspace_state.write().await;
    *guard = Some(state);

    console_log!("WASM client initialized successfully");
    Ok(())
}

/// Opens a messenger handle for the given CID.
/// This creates an ISM (InterSession Messaging) channel for reliable-ordered messaging.
/// Must be called once at login and maintained via polling (see ensure_messenger_open).
#[wasm_bindgen]
pub async fn open_messenger_for(cid_str: String) -> Result<(), JsValue> {
    let cid: u64 = cid_str
        .parse()
        .map_err(|e| JsValue::from_str(&format!("Invalid CID format: {}", e)))?;

    console_log!("Opening messenger handle for CID: {}", cid);

    // CRITICAL: Check if already open AND atomically mark as pending to prevent race conditions.
    // Clone Arc refs for use outside lock.
    let (already_open, is_pending, messenger, pending_opens, connections) = {
        let workspace_state = get_workspace_state();
        let guard = workspace_state.read().await;

        if let Some(state) = guard.as_ref() {
            // Check if messenger handle already exists for this CID
            if state.connections.contains_key(&cid) {
                (true, false, None, None, None)
            } else {
                // Try to atomically mark this CID as being opened
                let was_inserted = state.pending_opens.insert(cid);
                if !was_inserted {
                    // Another task is already opening this CID
                    (false, true, None, None, None)
                } else {
                    (
                        false,
                        false,
                        Some(state.messenger.clone()),
                        Some(state.pending_opens.clone()),
                        Some(state.connections.clone()),
                    )
                }
            }
        } else {
            return Err(JsValue::from_str("Workspace not initialized"));
        }
    }; // Lock released here

    if already_open {
        console_log!("Messenger handle already open for CID: {}", cid);
        return Ok(());
    }

    if is_pending {
        // Another task is opening this CID - wait briefly and check if it completed
        console_log!(
            "Messenger handle for CID: {} is being opened by another task, waiting...",
            cid
        );
        // Small delay then check if it's now in connections
        citadel_io::tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let workspace_state = get_workspace_state();
        let guard = workspace_state.read().await;
        if let Some(state) = guard.as_ref()
            && state.connections.contains_key(&cid)
        {
            console_log!("Messenger handle for CID: {} is now available", cid);
            return Ok(());
        }
        return Err(JsValue::from_str(
            "Messenger handle is being opened by another task",
        ));
    }

    // Now do the long multiplex operation WITHOUT holding the lock
    if let (Some(messenger), Some(pending_opens), Some(connections)) =
        (messenger, pending_opens, connections)
    {
        let result = messenger.multiplex(cid).await;

        match result {
            Ok(tx) => {
                connections.insert(cid, Arc::new(tx));
                pending_opens.remove(&cid);
                console_log!("Messenger handle opened for CID: {}", cid);
                Ok(())
            }
            Err(e) => {
                pending_opens.remove(&cid);
                Err(JsValue::from_str(&e.to_string()))
            }
        }
    } else {
        Err(JsValue::from_str("Workspace not initialized"))
    }
}

/// Ensures a messenger handle is open for the given CID.
/// Returns true if the messenger was just opened, false if already open or being opened by another task.
/// Use this for polling to maintain messenger handles across leader/follower tab transitions.
#[wasm_bindgen]
pub async fn ensure_messenger_open(cid_str: String) -> Result<bool, JsValue> {
    let cid: u64 = cid_str
        .parse()
        .map_err(|e| JsValue::from_str(&format!("Invalid CID format: {}", e)))?;

    // CRITICAL: Check if already open AND atomically mark as pending to prevent race conditions.
    // Two concurrent calls could both pass contains_key check before either inserts.
    // The pending_opens set prevents duplicate multiplex calls.
    let (already_open, is_pending, messenger, pending_opens, connections) = {
        let workspace_state = get_workspace_state();
        let guard = workspace_state.read().await;

        if let Some(state) = guard.as_ref() {
            // Check if messenger handle already exists for this CID
            if state.connections.contains_key(&cid) {
                // Already open, no action needed
                (true, false, None, None, None)
            } else {
                // Try to atomically mark this CID as being opened
                // insert() returns true if the value was newly inserted (not present before)
                let was_inserted = state.pending_opens.insert(cid);
                if !was_inserted {
                    // Another task is already opening this CID - don't duplicate
                    (false, true, None, None, None)
                } else {
                    // We successfully marked it as pending - we'll do the open
                    // Clone Arc refs for use outside lock
                    (
                        false,
                        false,
                        Some(state.messenger.clone()),
                        Some(state.pending_opens.clone()),
                        Some(state.connections.clone()),
                    )
                }
            }
        } else {
            return Err(JsValue::from_str("Workspace not initialized"));
        }
    }; // Lock released here

    if already_open {
        return Ok(false);
    }

    if is_pending {
        // Another task is opening this CID - we need to WAIT for it to complete
        // instead of returning immediately, otherwise the caller will try to send
        // messages before the handle is ready.
        console_log!(
            "Waiting for pending messenger open for CID: {} (another task is opening)",
            cid
        );

        // Poll until the handle is ready or the pending operation fails
        // Max wait: 30 seconds (multiplex can take time due to batched requests)
        let max_wait_ms = 30_000u64;
        let poll_interval_ms = 100u64;
        let max_iterations = max_wait_ms / poll_interval_ms;

        for iteration in 0..max_iterations {
            // Small delay before checking
            citadel_internal_service_connector::messenger::sleep_internal(
                std::time::Duration::from_millis(poll_interval_ms),
            )
            .await;

            // Check if handle is now available or pending is done
            let (is_ready, still_pending) = {
                let workspace_state = get_workspace_state();
                let guard = workspace_state.read().await;

                if let Some(state) = guard.as_ref() {
                    let ready = state.connections.contains_key(&cid);
                    let pending = state.pending_opens.contains(&cid);
                    (ready, pending)
                } else {
                    return Err(JsValue::from_str("Workspace not initialized"));
                }
            };

            if is_ready {
                console_log!(
                    "Messenger handle became ready for CID: {} (waited {}ms)",
                    cid,
                    (iteration + 1) * poll_interval_ms
                );
                return Ok(false); // Handle was opened by another task
            }

            if !still_pending {
                // Pending flag was cleared but handle not ready - the other task failed
                // Try to open it ourselves by recursing (will set pending flag again)
                console_log!(
                    "Pending cleared but handle not ready for CID: {}, retrying ourselves",
                    cid
                );
                return Box::pin(ensure_messenger_open(cid_str)).await;
            }

            // Still pending, continue waiting
            if iteration > 0 && iteration % 50 == 0 {
                console_log!(
                    "Still waiting for messenger open for CID: {} ({}ms elapsed)",
                    cid,
                    (iteration + 1) * poll_interval_ms
                );
            }
        }

        // Timeout - the other task is taking too long
        console_log!(
            "Timeout waiting for messenger open for CID: {} after {}ms",
            cid,
            max_wait_ms
        );
        return Err(JsValue::from_str(&format!(
            "Timeout waiting for messenger handle for CID: {}",
            cid
        )));
    }

    // Now do the long multiplex operation WITHOUT holding the lock
    // We have exclusive responsibility for this CID (marked in pending_opens)
    if let (Some(messenger), Some(pending_opens), Some(connections)) =
        (messenger, pending_opens, connections)
    {
        console_log!("Opening messenger handle for CID: {} (was missing)", cid);

        // Use a guard pattern to ensure we remove from pending_opens even on error
        let result = messenger.multiplex(cid).await;

        match result {
            Ok(tx) => {
                // Insert into connections
                connections.insert(cid, Arc::new(tx));
                // Remove from pending (we're done opening)
                pending_opens.remove(&cid);
                console_log!("Messenger handle opened for CID: {}", cid);
                Ok(true) // Messenger was just opened
            }
            Err(e) => {
                // Remove from pending on error so future attempts can retry
                pending_opens.remove(&cid);
                Err(JsValue::from_str(&e.to_string()))
            }
        }
    } else {
        Err(JsValue::from_str("Workspace not initialized"))
    }
}

/// Return the message stream to the shared workspace state after a
/// `next_message` read so the next call can reuse it.
///
/// `next_message` takes the stream out and releases the `RwLock` across
/// `recv().await`, so by the time we get here the workspace may have been torn
/// down (`close_connection` sets the state to `None`) or replaced (`restart`
/// installs a fresh state with its own stream) in response to a dropped
/// WebSocket. In either case our stream is obsolete and is simply dropped: we
/// only restore it when the slot is still present and empty. The previous code
/// `.expect("Workspace stream corrupted")`-ed on the `None` case, turning an
/// ordinary connection drop into a Rust panic (`RuntimeError: unreachable`) in
/// the WASM client.
/// Move everything still queued in `stream` onto `sink`, returning how many were
/// moved.
///
/// This exists as its own function because the alternative - letting the
/// receiver fall out of scope - is silent and lossy: a dropped tokio
/// `UnboundedReceiver` takes every message still queued in it, with no error and
/// no log. Order is preserved, so rescued messages stay ahead of traffic that
/// arrives after them.
fn drain_queued<T>(
    stream: &mut citadel_io::tokio::sync::mpsc::UnboundedReceiver<T>,
    sink: &mut std::collections::VecDeque<T>,
) -> usize {
    let mut moved = 0usize;
    while let Ok(msg) = stream.try_recv() {
        sink.push_back(msg);
        moved += 1;
    }
    moved
}

async fn restore_message_stream(
    mut stream: citadel_io::tokio::sync::mpsc::UnboundedReceiver<InternalServiceResponse>,
) {
    let mut guard = get_workspace_state().write().await;
    let Some(state) = guard.as_mut() else {
        // Workspace was torn down (`close_connection`) while we awaited the
        // message; our stream is obsolete. There is nowhere left to put its
        // contents, but say so rather than letting them vanish silently - these
        // are messages ILM already reported as delivered.
        let mut discarded = std::collections::VecDeque::new();
        let lost = drain_queued(&mut stream, &mut discarded);
        if lost > 0 {
            // Tagged ILM, not WASM. These lines describe the fate of messages
            // ILM has already reported as delivered, and the integration
            // harness captures console output by keyword — the list is
            // ['P2P', 'error', 'Error', 'ILM'], so a '[WASM]' prefix was
            // dropped before it reached any log. The diagnostic existed, ran,
            // and could never be read, which is why the reconnect loss stayed
            // unfalsifiable across several CI runs.
            console_log!(
                "[ILM-RESCUE] workspace torn down while {} delivered message(s) were still queued",
                lost
            );
        }
        return;
    };

    if state.stream.is_none() {
        state.stream.replace(stream);
        return;
    }

    // A newer stream replaced ours - `restart` installing a fresh state while we
    // awaited. Letting our receiver fall out of scope here discarded EVERY
    // message still queued in it, which is the reconnect message loss: ILM
    // reports the message delivered (it did send into the channel) and the
    // client never sees it, with no drop logged anywhere because it never
    // reached JS. Rescue them instead.
    let rescued = drain_queued(&mut stream, &mut state.pending);
    if rescued > 0 {
        console_log!(
            "[ILM-RESCUE] rescued {} queued message(s) from a replaced stream",
            rescued
        );
    }
}

#[wasm_bindgen]
pub async fn next_message() -> Result<JsValue, JsValue> {
    let workspace_state = get_workspace_state();
    let mut guard = workspace_state.write().await;

    // Take the stream so that the open_p2p_connection and send_p2p_connection functions
    // are not blocked while listening
    if let Some(state) = guard.as_mut() {
        // Rescued messages go out before anything new is awaited, so a reconnect
        // cannot leave them stranded behind later traffic.
        if let Some(response) = state.pending.pop_front() {
            return serialize_response(&response);
        }
        if let Some(mut stream) = state.stream.take() {
            drop(guard); // drop the guard to unblock
            if let Some(response) = stream.recv().await {
                // Convert to JsValue with custom BigInt handling for large CIDs
                restore_message_stream(stream).await;
                serialize_response(&response)
            } else {
                restore_message_stream(stream).await;
                Err(JsValue::from_str("Stream closed"))
            }
        } else {
            Err(JsValue::from_str(
                "next_message is already being called by another process",
            ))
        }
    } else {
        Err(JsValue::from_str("Workspace not initialized"))
    }
}

/// Convert string to SecurityLevel, matching TypeScript type:
/// 'Standard' | 'Reinforced' | 'High' | 'Extreme'
fn parse_security_level(s: Option<&str>) -> SecurityLevel {
    match s {
        Some("Standard") => SecurityLevel::Standard,
        Some("Reinforced") => SecurityLevel::Reinforced,
        Some("High") => SecurityLevel::High,
        Some("Extreme") => SecurityLevel::Extreme,
        _ => SecurityLevel::default(), // Falls back to Standard
    }
}

/// Sends a P2P message using ISM-routed reliable messaging.
/// Unlike send_p2p_message which bypasses ISM, this function uses
/// send_message_to_with_security_level for guaranteed delivery.
#[wasm_bindgen]
pub async fn send_p2p_message_reliable(
    local_cid_str: String,
    peer_cid_str: String,
    message: Vec<u8>,
    security_level: Option<String>,
) -> Result<(), JsValue> {
    let local_cid: u64 = local_cid_str
        .parse()
        .map_err(|e| JsValue::from_str(&format!("Invalid local CID format: {}", e)))?;
    let peer_cid: u64 = peer_cid_str
        .parse()
        .map_err(|e| JsValue::from_str(&format!("Invalid peer CID format: {}", e)))?;

    console_log!(
        "Sending reliable P2P message from {} to {}",
        local_cid,
        peer_cid
    );

    let sec_level = parse_security_level(security_level.as_deref());

    // CRITICAL: Clone the Arc handle and release the lock BEFORE the long async send.
    // Holding a read lock across await blocks next_message() which needs a write lock,
    // causing the JavaScript event loop to stall and the UI to freeze.
    let tx: Option<Arc<MessengerTx<CitadelWorkspaceBackend>>> = {
        let workspace_state = get_workspace_state();
        let guard = workspace_state.read().await;

        if let Some(state) = guard.as_ref() {
            state.connections.get(&local_cid).map(|r| r.value().clone())
        } else {
            return Err(JsValue::from_str("Workspace not initialized"));
        }
    }; // Lock released here

    if let Some(tx) = tx {
        // Use ISM-routed send for reliability - lock is NOT held during this await
        tx.send_message_to_with_security_level(peer_cid, sec_level, message)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        console_log!(
            "Reliable P2P message sent from {} to {}",
            local_cid,
            peer_cid
        );
        Ok(())
    } else {
        Err(JsValue::from_str(&format!(
            "No messaging handle found for local CID: {}. Call open_p2p_connection first.",
            local_cid
        )))
    }
}

/// Send one encoded media frame to a peer.
///
/// A dedicated binding rather than send_direct_to_internal_service, purely for
/// the hot path: frames arrive 30-60 times a second per track, and routing each
/// through a JsValue and serde-wasm-bindgen would deserialize a whole request
/// object — payload included — per frame. This takes the bytes directly and
/// builds the request in Rust.
///
/// Fire-and-forget by design. There is no ack to wait for; a frame that cannot
/// be queued is a frame worth dropping, because by the time a retry arrived it
/// would be too late to play.
#[wasm_bindgen]
pub fn send_media_frame(
    local_cid_str: String,
    peer_cid_str: String,
    track: u8,
    kind: u8,
    timestamp: u32,
    flags: u8,
    payload: Vec<u8>,
) -> Result<(), JsValue> {
    let local_cid: u64 = local_cid_str
        .parse()
        .map_err(|e| JsValue::from_str(&format!("Invalid local CID format: {}", e)))?;
    let peer_cid: u64 = peer_cid_str
        .parse()
        .map_err(|e| JsValue::from_str(&format!("Invalid peer CID format: {}", e)))?;

    let request = InternalServiceRequest::MediaSend {
        request_id: uuid::Uuid::new_v4(),
        cid: local_cid,
        peer_cid,
        track,
        kind,
        timestamp,
        flags,
        payload,
    };

    let Some(sink_tx) = sink_channel() else {
        return Err(JsValue::from_str("Client not properly initialized"));
    };

    sink_tx
        .send(InternalServicePayload::Request(request))
        .map_err(|e| JsValue::from_str(&format!("Failed to send media frame: {}", e)))
}

#[wasm_bindgen]
pub async fn send_direct_to_internal_service(message: JsValue) -> Result<(), JsValue> {
    // Note: Verbose logging removed to reduce console noise
    // Enable debug_log!() calls below for troubleshooting

    let request: InternalServiceRequest = deserialize_request(message)?;
    // debug_log!("Deserialized request: {:?}", request);

    // Use direct WebSocket channel instead of bypasser to avoid workspace lock
    if let Some(sink_tx) = sink_channel() {
        let payload = InternalServicePayload::Request(request);
        match sink_tx.send(payload) {
            Ok(()) => {
                // Success - no log needed to reduce noise
                Ok(())
            }
            Err(e) => {
                console_log!("Failed to send message to WebSocket channel: {:?}", e);
                Err(JsValue::from_str(&format!("Failed to send message: {}", e)))
            }
        }
    } else {
        console_log!("Sink channel not available - client not initialized");
        Err(JsValue::from_str("Client not properly initialized"))
    }
}

#[wasm_bindgen]
pub async fn close_connection() -> Result<(), JsValue> {
    console_log!("Closing WASM client connection");

    // Drop the sink first, so a subsequent connect installs a fresh one rather
    // than failing against a stale handle. Without this, reconnection was dead.
    if let Ok(mut guard) = SINK_CHANNEL.write() {
        *guard = None;
    }

    let workspace_state = get_workspace_state();
    let mut guard = workspace_state.write().await;

    if let Some(mut state) = guard.take() {
        // Close all P2P connections
        state.connections.clear();

        // Count what teardown is about to destroy, and say so.
        //
        // Dropping WorkspaceState drops the receiver AND `pending` — every
        // message ILM has already reported as delivered but that JavaScript has
        // not yet taken. Nothing logged that, and the [ILM-RESCUE] probe does
        // NOT cover it: that only runs when a `next_message` call had the
        // stream checked out at the moment it was replaced. During recovery the
        // loop is usually parked in backoff with the stream sitting in the
        // state, so this teardown is the common case and was entirely silent.
        //
        // Zero rescues therefore never meant zero loss, which is exactly the
        // wrong conclusion to invite from a diagnostic.
        let mut discarded = std::collections::VecDeque::new();
        let queued = state
            .stream
            .as_mut()
            .map(|stream| drain_queued(stream, &mut discarded))
            .unwrap_or(0);
        let already_pending = state.pending.len();
        if queued > 0 || already_pending > 0 {
            // ILM-prefixed so the integration harness's keyword filter keeps it.
            console_log!(
                "[ILM-TEARDOWN] closing with {} queued and {} pending message(s) still undelivered",
                queued,
                already_pending
            );
        }

        // Note: CitadelWorkspaceMessenger doesn't have a close() method
        // The connection will be cleaned up when the state is dropped
        console_log!("[ILM-TEARDOWN] WASM client connection closed");
        Ok(())
    } else {
        Err(JsValue::from_str("Workspace not initialized"))
    }
}

// Utility functions for TypeScript integration
#[wasm_bindgen]
pub fn get_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[wasm_bindgen]
pub fn is_initialized() -> bool {
    let workspace_state = get_workspace_state();
    // Use try_read to avoid blocking
    if let Ok(guard) = workspace_state.try_read() {
        guard.is_some()
    } else {
        false
    }
}

#[cfg(test)]
mod drain_queued_tests {
    use super::drain_queued;
    use std::collections::VecDeque;

    #[test]
    fn moves_every_queued_message_in_order() {
        // The regression: these are messages ILM has already reported as
        // delivered. Dropping the receiver loses them with no error anywhere.
        let (tx, mut rx) = citadel_io::tokio::sync::mpsc::unbounded_channel();
        for i in 0..3 {
            tx.send(i).unwrap();
        }
        let mut sink = VecDeque::new();
        assert_eq!(drain_queued(&mut rx, &mut sink), 3);
        // Order matters: rescued messages are replayed before newer traffic, so
        // reversing them here would reorder a conversation on every reconnect.
        assert_eq!(sink.into_iter().collect::<Vec<_>>(), vec![0, 1, 2]);
    }

    #[test]
    fn appends_rather_than_replacing_what_the_sink_already_holds() {
        // The sink is the live pending queue, which may already hold messages
        // rescued from an earlier replacement.
        let (tx, mut rx) = citadel_io::tokio::sync::mpsc::unbounded_channel();
        tx.send(9).unwrap();
        let mut sink: VecDeque<i32> = VecDeque::from(vec![7, 8]);
        assert_eq!(drain_queued(&mut rx, &mut sink), 1);
        assert_eq!(sink.into_iter().collect::<Vec<_>>(), vec![7, 8, 9]);
    }

    #[test]
    fn reports_nothing_moved_for_an_empty_queue() {
        // Guards the log line: claiming a rescue that did not happen would be
        // as misleading as the silent loss it replaced.
        let (_tx, mut rx) = citadel_io::tokio::sync::mpsc::unbounded_channel::<i32>();
        let mut sink = VecDeque::new();
        assert_eq!(drain_queued(&mut rx, &mut sink), 0);
        assert!(sink.is_empty());
    }
}

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use citadel_internal_service_connector::connector::InternalServiceConnector;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_connector::messenger::{CitadelWorkspaceMessenger, MessengerTx};
use citadel_internal_service_types::{
    InternalServicePayload, InternalServiceRequest, InternalServiceResponse,
};

use async_trait::async_trait;
use citadel_io::tokio::sync::{mpsc, RwLock};
use dashmap::DashMap;
use futures::{Sink, Stream};
// use send_wrapper::SendWrapper;  // Not needed with channel-based approach
use ws_stream_wasm::{WsMessage, WsMeta};
// use futures_util::{SinkExt as FuturesSinkExt, StreamExt as FuturesStreamExt};
use once_cell::sync::OnceCell;

// Custom serializer that handles u64 as BigInt
fn serialize_response_with_bigint(response: &InternalServiceResponse) -> Result<JsValue, JsValue> {
    // First convert to serde_json::Value to manipulate the structure
    let mut json_value = serde_json::to_value(response)
        .map_err(|e| JsValue::from_str(&format!("JSON serialization error: {}", e)))?;

    // Recursively convert u64 values that are too large for JavaScript numbers to strings
    convert_large_numbers_to_strings(&mut json_value);

    // Convert to JsValue with custom handling for large integers
    js_sys::JSON::parse(
        &serde_json::to_string(&json_value)
            .map_err(|e| JsValue::from_str(&format!("JSON string conversion error: {}", e)))?,
    )
    .map_err(|e| JsValue::from_str(&format!("JS JSON parse error: {:?}", e)))
}

// Custom deserializer that handles string CIDs from JavaScript
fn deserialize_request_with_string_cids(
    message: JsValue,
) -> Result<InternalServiceRequest, JsValue> {
    // First convert to JSON string, then to serde_json::Value for manipulation
    let json_str = js_sys::JSON::stringify(&message)
        .map_err(|e| JsValue::from_str(&format!("JSON stringify error: {:?}", e)))?;

    let json_str = json_str
        .as_string()
        .ok_or_else(|| JsValue::from_str("Failed to convert JSON to string"))?;

    let mut json_value: serde_json::Value = serde_json::from_str(&json_str)
        .map_err(|e| JsValue::from_str(&format!("JSON parse error: {}", e)))?;

    // Convert string CIDs back to numbers
    convert_string_cids_to_numbers(&mut json_value);

    // Convert to the final request type
    serde_json::from_value(json_value)
        .map_err(|e| JsValue::from_str(&format!("Request deserialization error: {}", e)))
}

// Recursively convert large integers to strings to avoid JavaScript overflow
fn convert_large_numbers_to_strings(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                // Check if the number exceeds JavaScript's safe integer range
                if u > (1u64 << 53) - 1 {
                    *value = serde_json::Value::String(u.to_string());
                }
            }
        }
        serde_json::Value::Object(map) => {
            for (_, v) in map.iter_mut() {
                convert_large_numbers_to_strings(v);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                convert_large_numbers_to_strings(v);
            }
        }
        _ => {}
    }
}

// Convert string CIDs back to numbers for Rust deserialization
fn convert_string_cids_to_numbers(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            // Handle specific CID field names
            for (key, v) in map.iter_mut() {
                if (key == "cid" || key == "peer_cid") && v.is_string() {
                    if let Some(s) = v.as_str() {
                        if let Ok(n) = s.parse::<u64>() {
                            *v = serde_json::Value::Number(serde_json::Number::from(n));
                        }
                    }
                } else {
                    convert_string_cids_to_numbers(v);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                convert_string_cids_to_numbers(v);
            }
        }
        _ => {}
    }
}
// use std::time::Duration;
use wasm_bindgen::prelude::*;

// WASM exports and logging setup
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

macro_rules! console_log {
    ($($t:tt)*) => (log(&format_args!($($t)*).to_string()))
}

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
static SINK_CHANNEL: OnceCell<mpsc::UnboundedSender<InternalServicePayload>> = OnceCell::new();

struct WorkspaceState {
    messenger: CitadelWorkspaceMessenger<CitadelWorkspaceBackend>,
    stream: citadel_io::tokio::sync::mpsc::UnboundedReceiver<InternalServiceResponse>,
    connections: Arc<DashMap<u64, MessengerTx<CitadelWorkspaceBackend>>>,
}

// Initialize the global state
fn get_workspace_state() -> &'static Arc<RwLock<Option<WorkspaceState>>> {
    WORKSPACE_STATE.get_or_init(|| Arc::new(RwLock::new(None)))
}

// Remove duplicate - already defined above

#[wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();
}

#[wasm_bindgen]
pub async fn init(ws_url: String) -> Result<(), JsValue> {
    console_log!("Initializing WASM client with URL: {}", ws_url);

    // Create channels for WebSocket communication
    let (sink_tx, mut sink_rx) = mpsc::unbounded_channel::<InternalServicePayload>();
    let (stream_tx, stream_rx) =
        mpsc::unbounded_channel::<std::io::Result<InternalServicePayload>>();

    // Store the sink channel globally for direct message sending
    SINK_CHANNEL
        .set(sink_tx.clone())
        .map_err(|_| JsValue::from_str("Failed to store sink channel"))?;

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
                // Handle outgoing messages from sink_rx
                Some(payload) = sink_rx.recv() => {
                    match serde_json::to_string(&payload) {
                        Ok(json) => {
                            console_log!("Sending JSON to server: {}", json);
                            let message = WsMessage::Text(json);
                            if let Err(e) = ws_sink.send(message).await {
                                console_log!("Error sending WebSocket message: {:?}", e);
                                break;
                            } else {
                                console_log!("JSON message sent successfully to server");
                            }
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
    });

    // Create IO implementation with channels
    let io_implementation = WasmWebSocketIO::new(sink_tx, stream_rx);

    let connector = InternalServiceConnector::from_io(io_implementation)
        .await
        .ok_or_else(|| JsValue::from_str("Failed to create connector"))?;

    let (messenger, stream) = CitadelWorkspaceMessenger::new(connector);
    let connections = Arc::new(DashMap::new());

    let state = WorkspaceState {
        messenger,
        stream,
        connections,
    };

    let workspace_state = get_workspace_state();
    let mut guard = workspace_state.write().await;
    *guard = Some(state);

    console_log!("WASM client initialized successfully");
    Ok(())
}

#[wasm_bindgen]
pub async fn open_p2p_connection(cid_str: String) -> Result<(), JsValue> {
    let cid: u64 = cid_str
        .parse()
        .map_err(|e| JsValue::from_str(&format!("Invalid CID format: {}", e)))?;

    console_log!("Opening P2P connection for CID: {}", cid);

    let workspace_state = get_workspace_state();
    let guard = workspace_state.read().await;

    if let Some(state) = guard.as_ref() {
        let tx = state
            .messenger
            .multiplex(cid)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        state.connections.insert(cid, tx);
        console_log!("P2P connection opened for CID: {}", cid);
        Ok(())
    } else {
        Err(JsValue::from_str("Workspace not initialized"))
    }
}

#[wasm_bindgen]
pub async fn next_message() -> Result<JsValue, JsValue> {
    let workspace_state = get_workspace_state();
    let mut guard = workspace_state.write().await;

    if let Some(state) = guard.as_mut() {
        if let Some(response) = state.stream.recv().await {
            // Convert to JsValue with custom BigInt handling for large CIDs
            serialize_response_with_bigint(&response)
        } else {
            Err(JsValue::from_str("Stream closed"))
        }
    } else {
        Err(JsValue::from_str("Workspace not initialized"))
    }
}

#[wasm_bindgen]
pub async fn send_p2p_message(cid_str: String, message: JsValue) -> Result<(), JsValue> {
    let cid: u64 = cid_str
        .parse()
        .map_err(|e| JsValue::from_str(&format!("Invalid CID format: {}", e)))?;

    console_log!("Sending P2P message to CID: {}", cid);

    let request: InternalServiceRequest = deserialize_request_with_string_cids(message)?;

    let workspace_state = get_workspace_state();
    let guard = workspace_state.read().await;

    if let Some(state) = guard.as_ref() {
        if let Some(tx) = state.connections.get(&cid) {
            tx.send_request(request)
                .await
                .map_err(|e| JsValue::from_str(&e.to_string()))?;
            console_log!("P2P message sent to CID: {}", cid);
            Ok(())
        } else {
            Err(JsValue::from_str(&format!(
                "No connection found for CID: {}",
                cid
            )))
        }
    } else {
        Err(JsValue::from_str("Workspace not initialized"))
    }
}

#[wasm_bindgen]
pub async fn send_direct_to_internal_service(message: JsValue) -> Result<(), JsValue> {
    console_log!("Sending direct message to internal service");
    console_log!(
        "Raw message from TypeScript: {:?}",
        message
            .as_string()
            .unwrap_or_else(|| "Not a string".to_string())
    );

    let request: InternalServiceRequest = deserialize_request_with_string_cids(message)?;
    console_log!("Deserialized request: {:?}", request);

    // Use direct WebSocket channel instead of bypasser to avoid workspace lock
    if let Some(sink_tx) = SINK_CHANNEL.get() {
        console_log!("Got direct sink channel, sending request via WebSocket");

        let payload = InternalServicePayload::Request(request);
        console_log!("Created payload, sending to WebSocket channel");

        match sink_tx.send(payload) {
            Ok(()) => {
                console_log!("Message sent successfully to WebSocket channel");
                console_log!("Direct message sent to internal service");
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

    let workspace_state = get_workspace_state();
    let mut guard = workspace_state.write().await;

    if let Some(state) = guard.take() {
        // Close all P2P connections
        state.connections.clear();

        // Note: CitadelWorkspaceMessenger doesn't have a close() method
        // The connection will be cleaned up when the state is dropped
        console_log!("WASM client connection closed");
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

use crate::io_interface::IOInterface;
use async_trait::async_trait;
use citadel_internal_service_types::InternalServicePayload;
use citadel_logging as log;
use futures::{Sink, SinkExt, Stream, StreamExt};
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use citadel_io::tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{
    accept_async,
    tungstenite::{Error as TungsteniteError, Message},
    WebSocketStream,
};

pub struct WebSocketInterface {
    listener: TcpListener,
}

impl WebSocketInterface {
    pub async fn new(addr: SocketAddr) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self { listener })
    }
}

#[async_trait]
impl IOInterface for WebSocketInterface {
    type Sink = WebSocketSink;
    type Stream = WebSocketStream_;

    async fn next_connection(&mut self) -> Option<(Self::Sink, Self::Stream)> {
        loop {
            match self.listener.accept().await {
                Ok((stream, addr)) => {
                    log::debug!(target: "citadel", "New WebSocket connection from {}", addr);

                    match accept_async(stream).await {
                        Ok(ws_stream) => {
                            let (sink, stream) = ws_stream.split();
                            return Some((
                                WebSocketSink { inner: sink },
                                WebSocketStream_ { inner: stream },
                            ));
                        }
                        Err(err) => {
                            log::error!(target: "citadel", "WebSocket handshake failed: {}", err);
                            continue;
                        }
                    }
                }
                Err(err) => {
                    log::error!(target: "citadel", "Failed to accept TCP connection: {}", err);
                    continue;
                }
            }
        }
    }
}

pub struct WebSocketSink {
    inner: futures_util::stream::SplitSink<WebSocketStream<TcpStream>, Message>,
}

impl Sink<InternalServicePayload> for WebSocketSink {
    type Error = std::io::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner)
            .poll_ready(cx)
            .map_err(websocket_error_to_io_error)
    }

    fn start_send(
        mut self: Pin<&mut Self>,
        item: InternalServicePayload,
    ) -> Result<(), Self::Error> {
        let serialized = serde_json::to_string(&item)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let message = Message::Text(serialized);
        Pin::new(&mut self.inner)
            .start_send(message)
            .map_err(websocket_error_to_io_error)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner)
            .poll_flush(cx)
            .map_err(websocket_error_to_io_error)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner)
            .poll_close(cx)
            .map_err(websocket_error_to_io_error)
    }
}

pub struct WebSocketStream_ {
    inner: futures_util::stream::SplitStream<WebSocketStream<TcpStream>>,
}

impl Stream for WebSocketStream_ {
    type Item = std::io::Result<InternalServicePayload>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match futures::ready!(Pin::new(&mut self.inner).poll_next(cx)) {
            Some(Ok(Message::Text(data))) => {
                match serde_json::from_str::<InternalServicePayload>(&data) {
                    Ok(payload) => Poll::Ready(Some(Ok(payload))),
                    Err(e) => {
                        log::error!(target: "citadel", "Failed to deserialize WebSocket JSON message: {}", e);
                        Poll::Ready(Some(Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            e,
                        ))))
                    }
                }
            }
            Some(Ok(Message::Binary(data))) => {
                // Fallback: try to parse binary data as JSON string
                match std::str::from_utf8(&data) {
                    Ok(text) => match serde_json::from_str::<InternalServicePayload>(text) {
                        Ok(payload) => Poll::Ready(Some(Ok(payload))),
                        Err(e) => {
                            log::error!(target: "citadel", "Failed to deserialize WebSocket binary message as JSON: {}", e);
                            Poll::Ready(Some(Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                e,
                            ))))
                        }
                    },
                    Err(e) => {
                        log::error!(target: "citadel", "WebSocket binary message is not valid UTF-8: {}", e);
                        Poll::Ready(Some(Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            e,
                        ))))
                    }
                }
            }
            Some(Ok(Message::Close(_))) => {
                log::debug!(target: "citadel", "WebSocket connection closed");
                Poll::Ready(None)
            }
            Some(Ok(msg)) => {
                log::warn!(target: "citadel", "Unexpected WebSocket message type: {:?}", msg);
                // Skip non-text/binary messages and continue
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Some(Err(e)) => {
                log::error!(target: "citadel", "WebSocket error: {}", e);
                Poll::Ready(Some(Err(websocket_error_to_io_error(e))))
            }
            None => {
                log::debug!(target: "citadel", "WebSocket stream ended");
                Poll::Ready(None)
            }
        }
    }
}

fn websocket_error_to_io_error(err: TungsteniteError) -> std::io::Error {
    match err {
        TungsteniteError::Io(io_err) => io_err,
        other => std::io::Error::new(std::io::ErrorKind::Other, other),
    }
}

// WebSocket client for testing
pub struct WebSocketClient {
    ws_stream: WebSocketStream<TcpStream>,
}

impl WebSocketClient {
    pub async fn connect(addr: SocketAddr) -> Result<Self, Box<dyn std::error::Error>> {
        let stream = TcpStream::connect(addr).await?;
        let url = format!("ws://{}/", addr);
        let (ws_stream, _) = tokio_tungstenite::client_async(url, stream).await?;
        Ok(Self { ws_stream })
    }

    pub async fn send(
        &mut self,
        payload: InternalServicePayload,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let serialized = serde_json::to_string(&payload)?;
        let message = Message::Text(serialized);
        self.ws_stream.send(message).await?;
        Ok(())
    }

    pub async fn send_json_string(
        &mut self,
        json_string: String,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let message = Message::Text(json_string);
        self.ws_stream.send(message).await?;
        Ok(())
    }

    pub async fn receive(
        &mut self,
    ) -> Result<Option<InternalServicePayload>, Box<dyn std::error::Error>> {
        if let Some(message) = self.ws_stream.next().await {
            match message? {
                Message::Text(data) => {
                    let payload = serde_json::from_str::<InternalServicePayload>(&data)?;
                    Ok(Some(payload))
                }
                Message::Binary(data) => {
                    // Fallback: try to parse binary data as JSON string
                    let text = std::str::from_utf8(&data)?;
                    let payload = serde_json::from_str::<InternalServicePayload>(text)?;
                    Ok(Some(payload))
                }
                Message::Close(_) => Ok(None),
                _ => Ok(None), // Skip other message types
            }
        } else {
            Ok(None)
        }
    }

    pub async fn receive_json_string(
        &mut self,
    ) -> Result<Option<String>, Box<dyn std::error::Error>> {
        if let Some(message) = self.ws_stream.next().await {
            match message? {
                Message::Text(data) => Ok(Some(data)),
                Message::Binary(data) => {
                    let text = std::str::from_utf8(&data)?;
                    Ok(Some(text.to_string()))
                }
                Message::Close(_) => Ok(None),
                _ => Ok(None), // Skip other message types
            }
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use citadel_internal_service_types::{
        InternalServiceRequest, InternalServiceResponse, SecBuffer,
    };
    use std::time::Duration;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_websocket_interface() {
        let addr = "127.0.0.1:0".parse().unwrap();
        let mut interface = WebSocketInterface::new(addr).await.unwrap();
        let bound_addr = interface.listener.local_addr().unwrap();

        // Spawn server task
        let server_task = tokio::spawn(async move {
            if let Some((mut sink, mut stream)) = interface.next_connection().await {
                // Echo received messages back
                while let Some(Ok(payload)) = stream.next().await {
                    log::info!(target: "citadel", "Server received: {:?}", payload);

                    // Echo back a response
                    let response = match payload {
                        InternalServicePayload::Request(InternalServiceRequest::Connect {
                            request_id,
                            ..
                        }) => InternalServicePayload::Response(
                            InternalServiceResponse::ConnectSuccess(
                                citadel_internal_service_types::ConnectSuccess {
                                    cid: 12345,
                                    request_id: Some(request_id),
                                },
                            ),
                        ),
                        _ => {
                            // Generic response for other requests
                            InternalServicePayload::Response(
                                InternalServiceResponse::ConnectSuccess(
                                    citadel_internal_service_types::ConnectSuccess {
                                        cid: 12345,
                                        request_id: None,
                                    },
                                ),
                            )
                        }
                    };

                    if let Err(e) = sink.send(response).await {
                        log::error!(target: "citadel", "Failed to send response: {}", e);
                        break;
                    }
                }
            }
        });

        // Give server time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Create client and connect
        let mut client = WebSocketClient::connect(bound_addr).await.unwrap();

        // Send a test message
        let request_id = Uuid::new_v4();
        let request = InternalServicePayload::Request(InternalServiceRequest::Connect {
            request_id,
            username: "test_user".to_string(),
            password: SecBuffer::from(b"password".to_vec()),
            connect_mode: Default::default(),
            udp_mode: Default::default(),
            keep_alive_timeout: Some(Duration::from_secs(30)),
            session_security_settings: Default::default(),
            server_password: None,
        });

        client.send(request).await.unwrap();

        // Receive response
        let response = client.receive().await.unwrap();
        assert!(response.is_some());

        match response.unwrap() {
            InternalServicePayload::Response(InternalServiceResponse::ConnectSuccess(success)) => {
                assert_eq!(success.cid, 12345);
                assert_eq!(success.request_id, Some(request_id));
            }
            _ => panic!("Expected ConnectSuccess response"),
        }

        // Clean up
        server_task.abort();
    }

    #[tokio::test]
    async fn test_websocket_json_format() {
        let addr = "127.0.0.1:0".parse().unwrap();
        let mut interface = WebSocketInterface::new(addr).await.unwrap();
        let bound_addr = interface.listener.local_addr().unwrap();

        // Spawn server task that echoes JSON
        let server_task = tokio::spawn(async move {
            if let Some((mut sink, mut stream)) = interface.next_connection().await {
                if let Some(Ok(payload)) = stream.next().await {
                    log::info!(target: "citadel", "Server received JSON payload: {:?}", payload);

                    // Send back a simple response
                    let response =
                        InternalServicePayload::Response(InternalServiceResponse::ConnectSuccess(
                            citadel_internal_service_types::ConnectSuccess {
                                cid: 99999,
                                request_id: None,
                            },
                        ));

                    if let Err(e) = sink.send(response).await {
                        log::error!(target: "citadel", "Failed to send response: {}", e);
                    }
                }
            }
        });

        // Give server time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Create client and test JSON format
        let mut client = WebSocketClient::connect(bound_addr).await.unwrap();

        // Create a proper request payload and serialize it to JSON
        let request_payload = InternalServicePayload::Request(InternalServiceRequest::Connect {
            request_id: Uuid::parse_str("123e4567-e89b-12d3-a456-426614174000").unwrap(),
            username: "frontend_user".to_string(),
            password: SecBuffer::from(b"password".to_vec()),
            connect_mode: Default::default(),
            udp_mode: Default::default(),
            keep_alive_timeout: Some(Duration::from_secs(30)),
            session_security_settings: Default::default(),
            server_password: None,
        });

        // Convert to JSON string to show what the frontend should send
        let json_payload = serde_json::to_string(&request_payload).unwrap();
        log::info!(target: "citadel", "Sending JSON payload: {}", json_payload);

        client.send_json_string(json_payload).await.unwrap();

        // Receive response as JSON string
        let response_json = client.receive_json_string().await.unwrap();
        assert!(response_json.is_some());

        let json_str = response_json.unwrap();
        log::info!(target: "citadel", "Received JSON response: {}", json_str);

        // Verify it's valid JSON and contains expected fields
        assert!(json_str.contains("\"Response\""));
        assert!(json_str.contains("\"ConnectSuccess\""));
        assert!(json_str.contains("\"cid\":99999"));

        // Also verify we can parse it back
        let parsed_response: InternalServicePayload = serde_json::from_str(&json_str).unwrap();
        match parsed_response {
            InternalServicePayload::Response(InternalServiceResponse::ConnectSuccess(success)) => {
                assert_eq!(success.cid, 99999);
            }
            _ => panic!("Expected ConnectSuccess response"),
        }

        // Clean up
        server_task.abort();
    }

    #[tokio::test]
    async fn test_websocket_multiple_messages() {
        let addr = "127.0.0.1:0".parse().unwrap();
        let mut interface = WebSocketInterface::new(addr).await.unwrap();
        let bound_addr = interface.listener.local_addr().unwrap();

        // Spawn server task that handles multiple messages
        let server_task = tokio::spawn(async move {
            if let Some((mut sink, mut stream)) = interface.next_connection().await {
                let mut message_count = 0;

                while let Some(Ok(payload)) = stream.next().await {
                    message_count += 1;
                    log::info!(target: "citadel", "Server received message {}: {:?}", message_count, payload);

                    // Send back a response with the message count
                    let response =
                        InternalServicePayload::Response(InternalServiceResponse::ConnectSuccess(
                            citadel_internal_service_types::ConnectSuccess {
                                cid: message_count,
                                request_id: None,
                            },
                        ));

                    if let Err(e) = sink.send(response).await {
                        log::error!(target: "citadel", "Failed to send response: {}", e);
                        break;
                    }

                    // Stop after 3 messages
                    if message_count >= 3 {
                        break;
                    }
                }
            }
        });

        // Give server time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Create client and send multiple messages
        let mut client = WebSocketClient::connect(bound_addr).await.unwrap();

        for i in 1..=3 {
            let request = InternalServicePayload::Request(InternalServiceRequest::Connect {
                request_id: Uuid::new_v4(),
                username: format!("test_user_{}", i),
                password: SecBuffer::from(b"password".to_vec()),
                connect_mode: Default::default(),
                udp_mode: Default::default(),
                keep_alive_timeout: Some(Duration::from_secs(30)),
                session_security_settings: Default::default(),
                server_password: None,
            });

            client.send(request).await.unwrap();

            // Receive response
            let response = client.receive().await.unwrap();
            assert!(response.is_some());

            match response.unwrap() {
                InternalServicePayload::Response(InternalServiceResponse::ConnectSuccess(
                    success,
                )) => {
                    assert_eq!(success.cid, i);
                }
                _ => panic!("Expected ConnectSuccess response"),
            }
        }

        // Clean up
        server_task.abort();
    }

    #[tokio::test]
    async fn test_websocket_connection_close() {
        let addr = "127.0.0.1:0".parse().unwrap();
        let mut interface = WebSocketInterface::new(addr).await.unwrap();
        let bound_addr = interface.listener.local_addr().unwrap();

        // Spawn server task
        let server_task = tokio::spawn(async move {
            if let Some((mut sink, mut stream)) = interface.next_connection().await {
                // Wait for one message then close
                if let Some(Ok(_payload)) = stream.next().await {
                    log::info!(target: "citadel", "Server received message, closing connection");
                    let _ = sink.close().await;
                }
            }
        });

        // Give server time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Create client and send a message
        let mut client = WebSocketClient::connect(bound_addr).await.unwrap();

        let request = InternalServicePayload::Request(InternalServiceRequest::Connect {
            request_id: Uuid::new_v4(),
            username: "test_user".to_string(),
            password: SecBuffer::from(b"password".to_vec()),
            connect_mode: Default::default(),
            udp_mode: Default::default(),
            keep_alive_timeout: Some(Duration::from_secs(30)),
            session_security_settings: Default::default(),
            server_password: None,
        });

        client.send(request).await.unwrap();

        // The connection should be closed by the server
        let response = client.receive().await.unwrap();
        assert!(response.is_none(), "Expected connection to be closed");

        // Clean up
        server_task.abort();
    }
}

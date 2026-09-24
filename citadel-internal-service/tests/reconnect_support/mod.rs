//! Helpers for `server_reconnect.rs`: a proxy that can cut the agent's link to its server
//! the way a deploy does, and the requests the tests make.

use crate::support::{expect, Stream, PASSWORD, TIMEOUT};
use citadel_internal_service_connector::connector::WrappedSink;
use citadel_internal_service_connector::io_interface::tcp::TcpIOInterface;
use citadel_internal_service_test_common as common;
use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
use futures::StreamExt;
use std::error::Error;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

/// Stands between the agent and the server. `sever` resets every live link at once, as a
/// Durable Object reset does ("Connection reset without closing handshake"); `refuse`
/// makes new links close as soon as they open.
pub struct Proxy {
    pub addr: SocketAddr,
    severed: tokio::sync::watch::Sender<u64>,
    accepting: Arc<AtomicBool>,
}

impl Proxy {
    pub async fn start(upstream: SocketAddr) -> Result<Self, Box<dyn Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (severed, _) = tokio::sync::watch::channel(0u64);
        let accepting = Arc::new(AtomicBool::new(true));
        let (severed_in, accepting_in) = (severed.clone(), accepting.clone());
        tokio::spawn(async move {
            while let Ok((inbound, _)) = listener.accept().await {
                if !accepting_in.load(Ordering::SeqCst) {
                    reset(inbound);
                    continue;
                }
                let mut sever = severed_in.subscribe();
                sever.mark_unchanged();
                tokio::spawn(async move {
                    let Ok(outbound) = TcpStream::connect(upstream).await else {
                        return reset(inbound);
                    };
                    let (mut inbound, mut outbound) = (inbound, outbound);
                    tokio::select! {
                        _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound) => {}
                        _ = sever.changed() => {}
                    }
                    reset(inbound);
                    reset(outbound);
                });
            }
        });
        Ok(Self {
            addr,
            severed,
            accepting,
        })
    }

    pub fn sever(&self) {
        self.severed.send_modify(|generation| *generation += 1);
    }

    pub fn refuse(&self, refuse: bool) {
        self.accepting.store(!refuse, Ordering::SeqCst);
    }
}

/// Close with RST rather than FIN.
///
/// A zero linger is the one setting that cannot block on drop (the deprecation is about
/// a non-zero one): the kernel discards the buffer and resets at once.
#[allow(deprecated)]
fn reset(stream: TcpStream) {
    let _ = stream.set_linger(Some(Duration::ZERO));
    drop(stream);
}

pub async fn disconnect(
    sink: &mut WrappedSink<TcpIOInterface>,
    stream: &mut Stream,
    cid: u64,
) -> Result<InternalServiceResponse, Box<dyn Error>> {
    let request_id = Uuid::new_v4();
    common::send(sink, InternalServiceRequest::Disconnect { request_id, cid }).await?;
    expect(stream, |response| {
        (response.request_id() == Some(&request_id)).then_some(response)
    })
    .await
}

pub async fn connect(
    sink: &mut WrappedSink<TcpIOInterface>,
    stream: &mut Stream,
    username: &str,
) -> Result<InternalServiceResponse, Box<dyn Error>> {
    let request_id = Uuid::new_v4();
    common::send(
        sink,
        InternalServiceRequest::Connect {
            request_id,
            username: username.to_string(),
            password: PASSWORD.as_bytes().to_vec().into(),
            connect_mode: Default::default(),
            udp_mode: Default::default(),
            keep_alive_timeout: None,
            session_security_settings: Default::default(),
            server_password: None,
        },
    )
    .await?;
    expect(stream, |response| {
        (response.request_id() == Some(&request_id)).then_some(response)
    })
    .await
}

/// The server answers over the session: proof the SDK session is live again.
pub async fn list_peers(
    sink: &mut WrappedSink<TcpIOInterface>,
    stream: &mut Stream,
    cid: u64,
) -> Result<InternalServiceResponse, Box<dyn Error>> {
    let request_id = Uuid::new_v4();
    common::send(
        sink,
        InternalServiceRequest::ListAllPeers { request_id, cid },
    )
    .await?;
    expect(stream, |response| {
        (response.request_id() == Some(&request_id)).then_some(response)
    })
    .await
}

/// Every link notification for `cid` that arrives within `window`.
pub async fn link_events(stream: &mut Stream, cid: u64, window: Duration) -> Vec<String> {
    let mut seen = Vec::new();
    let _ = tokio::time::timeout(window, async {
        while let Some(response) = stream.next().await {
            if let Some(event) = link_event(&response, cid) {
                seen.push(event);
            }
        }
    })
    .await;
    seen
}

/// The next link notification for `cid`, skipping anything else.
pub async fn next_link_event(stream: &mut Stream, cid: u64) -> Result<String, Box<dyn Error>> {
    tokio::time::timeout(TIMEOUT * 2, async {
        while let Some(response) = stream.next().await {
            if let Some(event) = link_event(&response, cid) {
                return Ok(event);
            }
        }
        Err("agent closed the connection".into())
    })
    .await?
}

fn link_event(response: &InternalServiceResponse, cid: u64) -> Option<String> {
    match response {
        InternalServiceResponse::ServerConnectionLost(lost) if lost.cid == cid => {
            Some(format!("lost(reconnecting={})", lost.reconnecting))
        }
        InternalServiceResponse::ServerReconnected(back) if back.cid == cid => {
            Some("reconnected".to_string())
        }
        InternalServiceResponse::ServerReconnectFailed(failed) if failed.cid == cid => {
            Some(format!("failed({})", failed.reason))
        }
        InternalServiceResponse::DisconnectNotification(gone)
            if gone.cid == cid && gone.request_id.is_none() =>
        {
            Some("disconnected".to_string())
        }
        _ => None,
    }
}

//! How the agent sends a frame of its own to a session's workspace server: over the session's
//! C2S link, the one its windows' messages use. Telemetry uploads (telemetry/uplink) and the
//! questions host egress asks about an install (kernel/apps/net/grants.rs) both go this way.

use crate::kernel::Connection;
use citadel_sdk::prelude::{Ratchet, SecurityLevel};
use futures::future::BoxFuture;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Sends one frame to a session's server over its C2S link.
pub trait SessionLink: Send + Sync {
    fn send_to_server(&self, cid: u64, frame: Vec<u8>) -> BoxFuture<'static, Result<(), String>>;
}

/// The agent's own frames are not content: the level every other agent-originated C2S frame
/// uses (kernel/conversations_io.rs).
const AGENT_FRAME_SECURITY_LEVEL: SecurityLevel = SecurityLevel::Standard;
/// Bounds one send, as a window's C2S message is bounded (requests/message.rs): a wedged
/// link fails the frame rather than stalling whatever waits behind it.
const SEND_TIMEOUT: Duration = crate::kernel::requests::message::PEER_SEND_TIMEOUT;

/// Every session's C2S sink, looked up per frame so a reconnect's new sink is used.
pub(crate) struct ServerLinks<R: Ratchet> {
    map: Arc<RwLock<HashMap<u64, Connection<R>>>>,
}

impl<R: Ratchet> ServerLinks<R> {
    pub(crate) fn new(map: Arc<RwLock<HashMap<u64, Connection<R>>>>) -> Self {
        Self { map }
    }
}

impl<R: Ratchet> SessionLink for ServerLinks<R> {
    fn send_to_server(&self, cid: u64, frame: Vec<u8>) -> BoxFuture<'static, Result<(), String>> {
        let sink = self
            .map
            .read()
            .get(&cid)
            .map(|conn| conn.sink_to_server.clone());
        Box::pin(async move {
            let sink = sink.ok_or_else(|| format!("session {cid} has no C2S link"))?;
            let sent = tokio::time::timeout(SEND_TIMEOUT, async {
                let mut sink = sink.lock().await;
                sink.set_security_level(AGENT_FRAME_SECURITY_LEVEL);
                sink.send(frame).await
            })
            .await;
            match sent {
                Ok(Ok(())) => Ok(()),
                Ok(Err(err)) => Err(format!("{err:?}")),
                Err(_) => Err(format!("send timed out after {SEND_TIMEOUT:?}")),
            }
        })
    }
}

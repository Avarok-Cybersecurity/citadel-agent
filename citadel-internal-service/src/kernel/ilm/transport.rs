//! ILM's network half for an agent-hosted ILM: the session's peer channels.
//!
//! Frames are encoded by `messenger::wire::encode_outbound`, the function the
//! browser's messenger uses, so a peer cannot tell which host sent a frame. The
//! bytes go out on `conn.peers[peer].sink` through `send_bounded`, the same
//! bounded send `requests/message.rs` performs for the browser's requests.
//!
//! Inbound frames arrive on the channel returned by [`AgentIlmTransport::new`];
//! nothing feeds it until a later phase routes a session's P2P reads here.
use crate::kernel::requests::message::{send_bounded, PeerSendError};
use crate::kernel::{AsyncSink, Connection};
use citadel_internal_service_connector::messenger::ilm::{
    NetworkError, Payload, UnderlyingSessionTransport,
};
use citadel_internal_service_connector::messenger::wire::{encode_outbound, FrameError};
use citadel_internal_service_connector::messenger::{InternalMessage, WrappedMessage};
use citadel_internal_service_types::SecurityLevel;
use citadel_sdk::prelude::{async_trait, Ratchet};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// The session's P2P channels, as the ILM transport needs them.
///
/// A trait so two ILMs can be run against each other without a network; the
/// production implementation is [`SessionPeerLinks`].
#[async_trait]
pub trait IlmPeerLinks: Send + Sync + 'static {
    async fn send_to_peer(
        &self,
        cid: u64,
        peer_cid: u64,
        security_level: SecurityLevel,
        frame: Vec<u8>,
    ) -> Result<(), String>;

    fn connected_peers(&self, cid: u64) -> Vec<u64>;
}

/// The agent's live sessions: `server_connection_map`.
pub struct SessionPeerLinks<R: Ratchet> {
    sessions: Arc<RwLock<HashMap<u64, Connection<R>>>>,
}

impl<R: Ratchet> SessionPeerLinks<R> {
    pub fn new(sessions: Arc<RwLock<HashMap<u64, Connection<R>>>>) -> Self {
        Self { sessions }
    }
}

#[async_trait]
impl<R: Ratchet> IlmPeerLinks for SessionPeerLinks<R> {
    async fn send_to_peer(
        &self,
        cid: u64,
        peer_cid: u64,
        security_level: SecurityLevel,
        frame: Vec<u8>,
    ) -> Result<(), String> {
        // Clone the sink out and drop the lock before any await, as
        // requests/message.rs does.
        let sink: AsyncSink<R> = {
            let sessions = self.sessions.read();
            let conn = sessions
                .get(&cid)
                .ok_or_else(|| format!("Connection for {cid} not found"))?;
            conn.peers
                .get(&peer_cid)
                .map(|peer| peer.sink.clone())
                .ok_or_else(|| format!("Peer connection for {peer_cid} not found"))?
        };
        send_bounded(&sink, security_level, frame)
            .await
            .map_err(|err| match err {
                PeerSendError::TimedOut => format!("Timed out waiting to send to {peer_cid}"),
                PeerSendError::Failed(err) => format!("Error sending message: {err}"),
            })
    }

    fn connected_peers(&self, cid: u64) -> Vec<u64> {
        let sessions = self.sessions.read();
        let mut peers: Vec<u64> = sessions
            .get(&cid)
            .map(|conn| conn.peers.keys().copied().collect())
            .unwrap_or_default();
        peers.sort_unstable();
        peers
    }
}

pub struct AgentIlmTransport<L: IlmPeerLinks> {
    cid: u64,
    links: L,
    inbound: tokio::sync::Mutex<UnboundedReceiver<InternalMessage>>,
}

impl<L: IlmPeerLinks> AgentIlmTransport<L> {
    /// The sender is the inbound path: decoded frames for this CID go in it.
    pub fn new(cid: u64, links: L) -> (Self, UnboundedSender<InternalMessage>) {
        let (tx, rx) = unbounded_channel();
        let transport = Self {
            cid,
            links,
            inbound: tokio::sync::Mutex::new(rx),
        };
        (transport, tx)
    }
}

#[async_trait]
impl<L: IlmPeerLinks> UnderlyingSessionTransport for AgentIlmTransport<L> {
    type Message = WrappedMessage;

    async fn next_message(&self) -> Option<Payload<WrappedMessage>> {
        self.inbound.lock().await.recv().await
    }

    /// Awaited in line, not spawned: a send that fails is reported to ILM,
    /// which keeps the message queued and retries it next cycle.
    async fn send_message(
        &self,
        message: Payload<WrappedMessage>,
    ) -> Result<(), NetworkError<Payload<WrappedMessage>>> {
        let frame = encode_outbound(message).map_err(|err| match err {
            FrameError::NotAMessageRequest(message) => NetworkError::SendFailed {
                reason: "an ILM message for a peer must carry a Message request".to_string(),
                message: *message,
            },
            FrameError::Serialize(err) => {
                NetworkError::ConnectionError(format!("Failed to frame ILM payload: {err}"))
            }
        })?;

        // An ILM hosted for one account sends only as that account.
        if frame.cid != self.cid {
            return Err(NetworkError::ConnectionError(format!(
                "ILM for {} refused to send as {}",
                self.cid, frame.cid
            )));
        }
        let peer_cid = frame.peer_cid.ok_or_else(|| {
            NetworkError::ConnectionError("an ILM frame must name a peer".to_string())
        })?;

        self.links
            .send_to_peer(self.cid, peer_cid, frame.security_level, frame.bytes)
            .await
            .map_err(NetworkError::ConnectionError)
    }

    async fn connected_peers(&self) -> Vec<u64> {
        self.links.connected_peers(self.cid)
    }

    fn local_id(&self) -> u64 {
        self.cid
    }
}

//! ILM's frames to and from the account's peer links.

use super::HostIo;
use async_trait::async_trait;
use citadel_internal_service_connector::messenger::{wire, WrappedMessage};
use intersession_layer_messaging::{
    InboundFrame, MessageMetadata, NetworkError, OutboundFrame, UnderlyingSessionTransport,
};
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;

pub(crate) struct AgentTransport {
    cid: u64,
    io: Arc<dyn HostIo>,
    inbound: Mutex<UnboundedReceiver<InboundFrame<WrappedMessage>>>,
}

impl AgentTransport {
    /// The transport, and the sender the peer read streams feed it through.
    pub(crate) fn new(
        cid: u64,
        io: Arc<dyn HostIo>,
    ) -> (Self, UnboundedSender<InboundFrame<WrappedMessage>>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (
            Self {
                cid,
                io,
                inbound: Mutex::new(rx),
            },
            tx,
        )
    }
}

#[async_trait]
impl UnderlyingSessionTransport for AgentTransport {
    type Message = WrappedMessage;

    async fn next_message(&self) -> Option<InboundFrame<Self::Message>> {
        self.inbound.lock().await.recv().await
    }

    /// Encoded exactly as a browser encodes it (the shared `wire` module), then
    /// put on the peer's link as a `Message` would be.
    async fn send_message(
        &self,
        frame: OutboundFrame<Self::Message>,
    ) -> Result<(), NetworkError<OutboundFrame<Self::Message>>> {
        let request = wire::to_request(frame).map_err(|err| {
            NetworkError::ConnectionError(format!("refusing an outgoing ILM frame: {err}"))
        })?;
        self.io
            .send_frame(request)
            .await
            .map_err(NetworkError::ConnectionError)
    }

    async fn connected_peers(&self) -> Vec<<Self::Message as MessageMetadata>::PeerId> {
        self.io.connected_peers(self.cid)
    }

    fn local_id(&self) -> <Self::Message as MessageMetadata>::PeerId {
        self.cid
    }
}

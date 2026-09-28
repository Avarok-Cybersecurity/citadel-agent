//! Counting what a messenger hands its agent, and the payloads to count.

// Shared by more than one test crate, each of which uses a subset.
#![allow(dead_code)]

use super::common::{get_free_port, register_and_connect_to_server_then_peers, PeerServiceHandles};
use citadel_crypt::ratchets::stacked::StackedRatchet;
use citadel_internal_service_connector::connector::InternalServiceConnector;
use citadel_internal_service_connector::io_interface::in_memory::InMemoryInterface;
use citadel_internal_service_connector::messenger::backend::CitadelWorkspaceBackend;
use citadel_internal_service_connector::messenger::{
    CitadelWorkspaceMessenger, CompressionHint, IlmOptions, MessengerTx,
};
use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
use citadel_io::tokio;
use citadel_io::tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

/// SDK framing per message, from the study's replica of the SDK packet
/// layout: length prefix, header, JustMessage wrapper, one AEAD layer, plus an
/// estimated TLS record and IPv4/TCP header. Each message also draws a
/// GROUP_HEADER_ACK of `SDK_ACK_WIRE` bytes. Used only to turn the measured
/// counts into an estimate; the counts themselves are measured.
pub const SDK_MESSAGE_OVERHEAD: usize = 4 + 64 + 80 + 32 + 22 + 40;
pub const SDK_ACK_WIRE: usize = 4 + 64 + 22 + 32 + 22 + 40;

#[derive(Default)]
pub struct Tally {
    messages: AtomicUsize,
    bytes: AtomicUsize,
}

impl Tally {
    pub fn read(&self) -> (usize, usize) {
        (
            self.messages.load(Ordering::SeqCst),
            self.bytes.load(Ordering::SeqCst),
        )
    }
}

/// Stand between a messenger and its agent, counting peer-bound messages.
pub fn tap(
    to_service: UnboundedSender<InternalServiceRequest>,
    tally: Arc<Tally>,
) -> UnboundedSender<InternalServiceRequest> {
    let (tx, mut rx) = unbounded_channel::<InternalServiceRequest>();
    drop(tokio::spawn(async move {
        while let Some(request) = rx.recv().await {
            if let InternalServiceRequest::Message {
                message,
                peer_cid: Some(_),
                ..
            } = &request
            {
                tally.messages.fetch_add(1, Ordering::SeqCst);
                tally.bytes.fetch_add(message.len(), Ordering::SeqCst);
            }
            if to_service.send(request).is_err() {
                return;
            }
        }
    }));
    tx
}

pub fn fixtures(prefix: &str) -> Vec<Vec<u8>> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/traffic");
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .expect("fixture dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .into_string()
                .expect("utf8")
        })
        .filter(|name| name.starts_with(prefix))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| std::fs::read(format!("{dir}/{name}")).expect("fixture"))
        .collect()
}

pub type Rx = UnboundedReceiver<InternalServiceResponse>;
pub type Tx = MessengerTx<CitadelWorkspaceBackend>;

/// One agent and the messenger in front of it.
pub struct Side {
    pub messenger: CitadelWorkspaceMessenger<CitadelWorkspaceBackend>,
    pub tx: Tx,
    pub rx: Rx,
    pub cid: u64,
    /// The raw bytes of every peer message this side's agent delivered to it,
    /// before the messenger decoded anything: what an older build would see.
    pub arrived: Arc<Mutex<Vec<Vec<u8>>>>,
}

/// Record the raw bytes of each peer message on its way to the messenger.
fn observe(mut from_service: Rx, arrived: Arc<Mutex<Vec<Vec<u8>>>>) -> Rx {
    let (tx, rx) = unbounded_channel();
    drop(tokio::spawn(async move {
        while let Some(response) = from_service.recv().await {
            if let InternalServiceResponse::MessageNotification(n) = &response {
                arrived.lock().expect("lock").push(n.message.clone());
            }
            if tx.send(response).is_err() {
                return;
            }
        }
    }));
    rx
}

/// Two registered, P2P-connected agents with a messenger each, counted.
pub async fn two_agents(a: IlmOptions, b: IlmOptions, tally: &Arc<Tally>) -> (Side, Side) {
    let addresses: Vec<SocketAddr> = (0..2)
        .map(|_| {
            format!("127.0.0.1:{}", get_free_port())
                .parse()
                .expect("addr")
        })
        .collect();
    let mut handles =
        register_and_connect_to_server_then_peers::<StackedRatchet>(addresses, None, None)
            .await
            .expect("two connected peers");
    let mut sides = Vec::new();
    for options in [a, b] {
        let (to_service, from_service, cid) = handles.take_next_service_handle();
        let arrived = Arc::new(Mutex::new(Vec::new()));
        let io = InMemoryInterface::from_request_response_pair(
            tap(to_service, tally.clone()),
            observe(from_service, arrived.clone()),
        );
        let connector = InternalServiceConnector::from_io(io)
            .await
            .expect("connector");
        let (messenger, rx) = CitadelWorkspaceMessenger::new(connector, options);
        let tx = messenger.multiplex(cid).await.expect("multiplex");
        sides.push(Side {
            messenger,
            tx,
            rx,
            cid,
            arrived,
        });
    }
    let b = sides.pop().expect("b");
    let a = sides.pop().expect("a");
    a.tx.wait_for_peer_to_connect(b.cid)
        .await
        .expect("a sees b");
    b.tx.wait_for_peer_to_connect(a.cid)
        .await
        .expect("b sees a");
    (a, b)
}

pub async fn expect(rx: &mut Rx, from: u64, payload: &[u8]) {
    let wait = async {
        loop {
            if let Some(InternalServiceResponse::MessageNotification(n)) = rx.recv().await {
                if n.peer_cid == from && n.message == payload {
                    return;
                }
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(30), wait)
        .await
        .expect("the payload arrived intact");
}

pub async fn send(tx: &Tx, to: u64, payload: &[u8], hint: CompressionHint) {
    tx.send_message_to_with_hint(to, Default::default(), Uuid::new_v4(), payload, Some(hint))
        .await
        .expect("send");
}

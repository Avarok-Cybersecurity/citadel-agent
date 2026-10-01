//! Two agents on one server, P2P-connected: Alice's agent hosts her ILM, Bob's
//! window runs ILM itself (the browser's messenger, natively).

use crate::group::recv_until;
use crate::{get_free_port, register_and_connect_to_server_then_peers, PeerHandle};
use citadel_internal_service_connector::connector::InternalServiceConnector;
use citadel_internal_service_connector::io_interface::in_memory::InMemoryInterface;
use citadel_internal_service_connector::messenger::backend::CitadelWorkspaceBackend;
use citadel_internal_service_connector::messenger::{
    CitadelWorkspaceMessenger, IlmOptions, MessengerTx,
};
use citadel_internal_service_types::{
    ClientCapabilities, ConfigCommand, InternalServiceRequest, InternalServiceResponse,
    SecurityLevel,
};
use citadel_sdk::prelude::StackedRatchet;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use uuid::Uuid;

pub type BrowserTx = MessengerTx<CitadelWorkspaceBackend>;

pub struct BrowserSide {
    pub messenger: CitadelWorkspaceMessenger<CitadelWorkspaceBackend>,
    pub tx: BrowserTx,
    pub rx: UnboundedReceiver<InternalServiceResponse>,
    pub cid: u64,
    /// Raw bytes of every peer message Bob's agent handed his window.
    pub arrived: Arc<Mutex<Vec<Vec<u8>>>>,
}

pub struct MixedHosting {
    pub alice_addr: SocketAddr,
    pub alice: PeerHandle,
    pub bob: BrowserSide,
}

pub fn declare_agent_ilm() -> InternalServiceRequest {
    InternalServiceRequest::ConnectionManagement {
        request_id: Uuid::new_v4(),
        management_command: ConfigCommand::DeclareCapabilities {
            capabilities: ClientCapabilities { agent_ilm: true },
        },
    }
}

/// Declare on `alice`'s window that the agent hosts her ILM, and wait for the answer.
pub async fn host_alice(alice: &mut PeerHandle) {
    alice.0.send(declare_agent_ilm()).expect("alice open");
    recv_until(&mut alice.1, "capabilities", |r| {
        matches!(r, InternalServiceResponse::AgentCapabilities(_))
    })
    .await;
}

pub async fn mixed_hosting(bob_options: IlmOptions) -> MixedHosting {
    let addresses: Vec<SocketAddr> = (0..2)
        .map(|_| {
            format!("127.0.0.1:{}", get_free_port())
                .parse()
                .expect("addr")
        })
        .collect();
    let mut handles =
        register_and_connect_to_server_then_peers::<StackedRatchet>(addresses.clone(), None, None)
            .await
            .expect("two connected peers");
    let (bob_to_service, bob_from_service, bob_cid) = handles.remove(1);
    let mut alice = handles.remove(0);

    let arrived = Arc::new(Mutex::new(Vec::new()));
    let observed = {
        let (tx, rx) = unbounded_channel();
        let arrived = arrived.clone();
        let mut from = bob_from_service;
        tokio::spawn(async move {
            while let Some(response) = from.recv().await {
                if let InternalServiceResponse::MessageNotification(n) = &response {
                    arrived.lock().expect("lock").push(n.message.clone());
                }
                if tx.send(response).is_err() {
                    return;
                }
            }
        });
        rx
    };
    let io = InMemoryInterface::from_request_response_pair(bob_to_service, observed);
    let connector = InternalServiceConnector::from_io(io)
        .await
        .expect("connector");
    let (messenger, rx) = CitadelWorkspaceMessenger::new(connector, bob_options);
    let tx = messenger.multiplex(bob_cid).await.expect("multiplex");
    tx.wait_for_peer_to_connect(alice.2)
        .await
        .expect("bob sees alice");

    host_alice(&mut alice).await;
    MixedHosting {
        alice_addr: addresses[0],
        alice,
        bob: BrowserSide {
            messenger,
            tx,
            rx,
            cid: bob_cid,
            arrived,
        },
    }
}

/// Alice sends through her agent's ILM; `Ok` once ILM has the message.
pub async fn alice_sends(alice: &mut PeerHandle, bob: u64, body: &[u8]) {
    let request_id = Uuid::new_v4();
    alice
        .0
        .send(InternalServiceRequest::SendReliable {
            request_id,
            cid: alice.2,
            peer_cid: bob,
            message: body.to_vec(),
            security_level: SecurityLevel::Standard,
            compression_hint: Some("text".to_string()),
        })
        .expect("alice open");
    let answer = recv_until(&mut alice.1, "the send's answer", |r| {
        r.request_id() == Some(&request_id)
    })
    .await;
    assert!(
        matches!(answer, InternalServiceResponse::SendReliableAccepted(_)),
        "the agent's ILM refused the message: {answer:?}"
    );
}

/// The next application message from `from` on `rx`, skipping everything else.
pub async fn next_message(
    rx: &mut UnboundedReceiver<InternalServiceResponse>,
    from: u64,
) -> Vec<u8> {
    match recv_until(
        rx,
        "a message",
        |r| matches!(r, InternalServiceResponse::MessageNotification(n) if n.peer_cid == from),
    )
    .await
    {
        InternalServiceResponse::MessageNotification(n) => n.message,
        _ => unreachable!(),
    }
}

/// Everything that arrives on `rx` within `settle`.
pub async fn quiet(
    rx: &mut UnboundedReceiver<InternalServiceResponse>,
    settle: Duration,
) -> Vec<InternalServiceResponse> {
    let mut seen = Vec::new();
    while let Ok(Some(r)) = tokio::time::timeout(settle, rx.recv()).await {
        seen.push(r);
    }
    seen
}

/// The username the agent at `addr` holds `cid` under, asked as any page would.
pub async fn username_of(addr: SocketAddr, cid: u64) -> String {
    let mut window = crate::open_localhost_connection(addr)
        .await
        .expect("a window");
    let request_id = Uuid::new_v4();
    window
        .0
        .send(InternalServiceRequest::GetSessions { request_id })
        .expect("open");
    match recv_until(&mut window.1, "sessions", |r| {
        r.request_id() == Some(&request_id)
    })
    .await
    {
        InternalServiceResponse::GetSessionsResponse(sessions) => sessions
            .sessions
            .into_iter()
            .find(|s| s.cid == cid)
            .map(|s| s.username)
            .expect("the session is listed"),
        other => panic!("no session list: {other:?}"),
    }
}

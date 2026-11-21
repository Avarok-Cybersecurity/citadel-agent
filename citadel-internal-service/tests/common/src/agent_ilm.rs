//! Two agents on one server, P2P-connected. Each hosts its account's ILM, as
//! every agent does for every signed-in account; neither window declares.

use crate::group::recv_until;
use crate::{
    connect_every_pair, get_free_port, server_info_skip_cert_verification,
    services_connected_to_server_at, PeerHandle,
};
use citadel_internal_service_connector::io_interface::tcp::TcpIOInterface;
use citadel_internal_service_types::{
    AccountPreferences, ChatSecurityLevel, ClientCapabilities, ConfigCommand,
    ConversationEventKind, ConversationMessage, InternalServiceRequest, InternalServiceResponse,
    MessageType, PeerSecurityMinimum, SecurityLevel,
};
use citadel_sdk::prelude::StackedRatchet;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

/// What a window that hosts no Citadel apps declares. Nothing about hosting:
/// the agent hosts every account's ILM whether or not a window declares.
pub fn declaration() -> InternalServiceRequest {
    InternalServiceRequest::ConnectionManagement {
        request_id: Uuid::new_v4(),
        management_command: ConfigCommand::DeclareCapabilities {
            capabilities: ClientCapabilities { ihttp: false },
        },
    }
}

/// `cid` pauses `peer` as the UI's "Pause connection" does: a record the
/// agent, which answers `cid`'s offers, reads before each answer.
pub fn pause(cid: u64, peer: u64) -> InternalServiceRequest {
    InternalServiceRequest::LocalDBSetKV {
        request_id: Uuid::new_v4(),
        cid,
        peer_cid: None,
        key: format!("p2p_paused_peer_{peer}"),
        value: b"paused".to_vec(),
    }
}

/// `cid` requires at least `level` of `peer`; the agent declines a lower offer.
pub fn require_level(cid: u64, peer: u64, level: ChatSecurityLevel) -> InternalServiceRequest {
    let preferences = AccountPreferences {
        security_minimums: vec![PeerSecurityMinimum {
            peer_cid: peer,
            level,
        }],
        ..AccountPreferences::UI_DEFAULTS
    };
    InternalServiceRequest::SetAccountPreferences {
        request_id: Uuid::new_v4(),
        cid,
        preferences: Box::new(preferences),
    }
}

/// Send `request` and wait for the answer carrying its id.
pub async fn answered(
    tx: &UnboundedSender<InternalServiceRequest>,
    rx: &mut UnboundedReceiver<InternalServiceResponse>,
    request: InternalServiceRequest,
) -> InternalServiceResponse {
    let id = *request.request_id().expect("a request id");
    tx.send(request).expect("the window is open");
    recv_until(rx, "the answer", |r| r.request_id() == Some(&id)).await
}

pub struct HostedPair {
    pub alice_addr: SocketAddr,
    pub alice: PeerHandle,
    pub bob: PeerHandle,
}

pub async fn hosted_pair() -> HostedPair {
    let (server, server_addr) = server_info_skip_cert_verification::<StackedRatchet>();
    tokio::task::spawn(server);
    let addresses: Vec<SocketAddr> = (0..2)
        .map(|_| {
            format!("127.0.0.1:{}", get_free_port())
                .parse()
                .expect("addr")
        })
        .collect();
    let services = services_connected_to_server_at::<StackedRatchet, TcpIOInterface>(
        addresses.clone(),
        server_addr,
        None,
    )
    .await
    .expect("two server-connected agents");
    let mut handles = connect_every_pair(services, None, Default::default())
        .await
        .expect("two connected peers");
    let bob = handles.remove(1);
    let alice = handles.remove(0);
    HostedPair {
        alice_addr: addresses[0],
        alice,
        bob,
    }
}

/// `from` sends through its agent's ILM; returns once ILM has the message.
pub async fn sends(from: &mut PeerHandle, to: u64, body: &[u8]) {
    let request_id = Uuid::new_v4();
    from.0
        .send(InternalServiceRequest::SendReliable {
            request_id,
            cid: from.2,
            peer_cid: to,
            message: body.to_vec(),
            security_level: SecurityLevel::Standard,
            compression_hint: Some("text".to_string()),
        })
        .expect("the window is open");
    let answer = recv_until(&mut from.1, "the send's answer", |r| {
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

/// A text `ConversationSend` from `cid` to `peer`, and the request id it carries.
pub fn conversation_send(cid: u64, peer: u64, content: &str) -> (Uuid, InternalServiceRequest) {
    let request_id = Uuid::new_v4();
    let request = InternalServiceRequest::ConversationSend {
        request_id,
        cid,
        peer_cid: peer,
        content: content.into(),
        message_type: MessageType::Text,
        reply_to: None,
        mentions: None,
        attachments: None,
        document_id: None,
        document_title: None,
        security_level: SecurityLevel::Standard,
    };
    (request_id, request)
}

/// The next `ConversationEvent` of `kind` on `rx` whose message passes `keep`.
pub async fn event_of(
    rx: &mut UnboundedReceiver<InternalServiceResponse>,
    kind: ConversationEventKind,
    keep: impl Fn(&ConversationMessage) -> bool,
) -> ConversationMessage {
    let event = recv_until(rx, "the event", |r| {
        matches!(r, InternalServiceResponse::ConversationEvent(e)
            if e.kind == kind && e.message.as_ref().is_some_and(&keep))
    })
    .await;
    let InternalServiceResponse::ConversationEvent(event) = event else {
        unreachable!()
    };
    event.message.expect("the message")
}

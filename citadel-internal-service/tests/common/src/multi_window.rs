//! Several localhost connections attached to one session: what several browser
//! windows (or a browser and the installed PWA) are to the agent.

use crate::group::recv_until;
use crate::{open_localhost_connection, two_sessions_on_one_service_at, PeerHandle};
use citadel_internal_service_types::{
    AttachProof, ConfigCommand, InternalServiceRequest, InternalServiceResponse, SessionRole,
};
use citadel_sdk::prelude::*;
use std::error::Error;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

/// A window: a localhost connection with no session of its own.
pub type Window = (
    UnboundedSender<InternalServiceRequest>,
    UnboundedReceiver<InternalServiceResponse>,
);

/// Alice (session 0, `secret_0`) and Bob (session 1) on one agent, P2P-connected,
/// plus the agent's address so further windows can be opened onto it.
pub struct TwoAccounts {
    pub addr: SocketAddr,
    pub alice: PeerHandle,
    pub bob: PeerHandle,
}

pub const ALICE_PASSWORD: &str = "secret_0";

pub async fn two_connected_accounts(tag: &str) -> Result<TwoAccounts, Box<dyn Error>> {
    let (addr, mut alice, mut bob) = two_sessions_on_one_service_at(tag).await?;
    crate::register_p2p(
        &mut alice.0,
        &mut alice.1,
        alice.2,
        &mut bob.0,
        &mut bob.1,
        bob.2,
        SessionSecuritySettings::default(),
        None::<PreSharedKey>,
    )
    .await?;
    crate::connect_p2p(
        &mut alice.0,
        &mut alice.1,
        alice.2,
        &mut bob.0,
        &mut bob.1,
        bob.2,
        SessionSecuritySettings::default(),
        None::<PreSharedKey>,
    )
    .await?;
    Ok(TwoAccounts { addr, alice, bob })
}

pub async fn open_window(addr: SocketAddr) -> Result<Window, Box<dyn Error>> {
    open_localhost_connection(addr).await
}

pub fn attach_request(cid: u64, proof: AttachProof) -> InternalServiceRequest {
    InternalServiceRequest::ConnectionManagement {
        request_id: Uuid::new_v4(),
        management_command: ConfigCommand::AttachSession {
            session_cid: cid,
            proof,
        },
    }
}

pub fn password(text: &str) -> AttachProof {
    AttachProof::Password(SecBuffer::from(text.as_bytes().to_vec()))
}

/// The answer to an `AttachSession`: `Ok((role, token))` or the refusal text.
pub async fn attach(
    window: &mut Window,
    cid: u64,
    proof: AttachProof,
) -> Result<(SessionRole, Vec<u8>), String> {
    window
        .0
        .send(attach_request(cid, proof))
        .expect("window open");
    match recv_until(&mut window.1, "attach answer", |r| {
        matches!(
            r,
            InternalServiceResponse::SessionAttached(_)
                | InternalServiceResponse::ConnectionManagementFailure(_)
        )
    })
    .await
    {
        InternalServiceResponse::SessionAttached(ok) => Ok((ok.role, ok.token)),
        InternalServiceResponse::ConnectionManagementFailure(fail) => Err(fail.error),
        _ => unreachable!(),
    }
}

/// Bob sends Alice one P2P message, raw (no ILM framing), and returns its bytes.
pub fn bob_messages_alice(bob: &PeerHandle, alice: u64, text: &str) -> Vec<u8> {
    let body = text.as_bytes().to_vec();
    bob.0
        .send(InternalServiceRequest::Message {
            request_id: Uuid::new_v4(),
            message: body.clone(),
            cid: bob.2,
            peer_cid: Some(alice),
            security_level: SecurityLevel::Standard,
        })
        .expect("bob open");
    body
}

/// Read until Alice's copy of `body` arrives, failing on anything that says a
/// response meant for some other connection reached this one.
pub async fn receives_message(
    rx: &mut UnboundedReceiver<InternalServiceResponse>,
    who: &str,
    alice: u64,
    body: &[u8],
) {
    let got = recv_until(
        rx,
        who,
        |r| matches!(r, InternalServiceResponse::MessageNotification(n) if n.message == body),
    )
    .await;
    let InternalServiceResponse::MessageNotification(n) = got else {
        unreachable!()
    };
    assert_eq!(
        n.cid, alice,
        "{who}: the message arrived for the wrong session"
    );
}

/// Everything already queued on `rx`, waiting `settle` for stragglers.
pub async fn drain(
    rx: &mut UnboundedReceiver<InternalServiceResponse>,
    settle: Duration,
) -> Vec<InternalServiceResponse> {
    let mut seen = Vec::new();
    while let Ok(Some(r)) = tokio::time::timeout(settle, rx.recv()).await {
        seen.push(r);
    }
    seen
}

/// The next role notification for `cid`.
pub async fn next_role(
    rx: &mut UnboundedReceiver<InternalServiceResponse>,
    cid: u64,
) -> (SessionRole, u32) {
    match recv_until(
        rx,
        "role notification",
        |r| matches!(r, InternalServiceResponse::SessionRoleNotification(n) if n.cid == cid),
    )
    .await
    {
        InternalServiceResponse::SessionRoleNotification(n) => (n.role, n.attached),
        _ => unreachable!(),
    }
}

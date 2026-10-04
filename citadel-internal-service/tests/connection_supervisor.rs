//! The agent keeps an account's links alive with no window open.
//!
//! Over the real SDK: two agents on one in-process server. `a` reaches it through a
//! `Proxy`, so its link can be cut or can die silently, and is supervised by the policy the
//! shipped agent runs. `b` is hosted, answers for its account, and does not supervise: it is
//! the peer the supervised agent has to find again by itself.

#[allow(dead_code)]
#[path = "reconnect_support/mod.rs"]
mod reconnect;
#[allow(dead_code)]
#[path = "supervisor_support/mod.rs"]
mod supervised;
#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service::AGENT_SUPERVISOR;
use citadel_internal_service_test_common as common;
use citadel_internal_service_types::{
    ConfigCommand, InternalServiceRequest, InternalServiceResponse, MessageStatus, MessageType,
    SecurityLevel, SupervisorState,
};
use std::error::Error;
use std::time::{Duration, Instant};
use supervised::{pair, peer_connect};
use support::expect;
use uuid::Uuid;

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after 1970")
        .as_millis() as u64
}

/// The point of the supervisor: a path that dies without a word is noticed in seconds, where
/// the protocol keep-alive takes 45 minutes.
const SILENT_DEATH_BUDGET: Duration = Duration::from_secs(30);

/// A path that dies silently is noticed, and the windows are told, within the budget. What
/// follows (the SDK ending the dead session locally, so the agent can reconnect at once)
/// needs an SDK call that does not exist yet, so the path is then let go and the usual
/// reconnect brings the link back.
#[tokio::test(flavor = "multi_thread")]
async fn a_silent_dead_path_is_noticed_within_thirty_seconds() -> Result<(), Box<dyn Error>> {
    let mut pair = pair(Some(AGENT_SUPERVISOR), None).await?;
    let started = Instant::now();
    pair.proxy.stall();
    let told = tokio::time::timeout(
        SILENT_DEATH_BUDGET,
        expect(&mut pair.a.stream, |response| match response {
            InternalServiceResponse::SupervisorNotification(n) => Some((n.peer_cid, n.state)),
            _ => None,
        }),
    )
    .await
    .map_err(|_| format!("a dead path went unnoticed for {SILENT_DEATH_BUDGET:?}"))??;
    eprintln!("noticed after {:?}", started.elapsed());
    assert_eq!(
        told,
        (None, SupervisorState::Healing),
        "the windows are told the account's link is being healed"
    );
    pair.proxy.release();
    let mut told = Vec::new();
    expect(&mut pair.a.stream, |response| match response {
        InternalServiceResponse::SupervisorNotification(n) => {
            told.push((n.peer_cid, n.state));
            ((n.peer_cid, n.state) == (None, SupervisorState::Healed)).then_some(())
        }
        _ => None,
    })
    .await?;
    let peers = reconnect::list_peers(&mut pair.a.sink, &mut pair.a.stream, pair.a.cid).await?;
    assert!(
        matches!(peers, InternalServiceResponse::ListAllPeersResponse(_)),
        "the link is back and carries traffic: {peers:?} after {told:?}"
    );
    Ok(())
}

async fn connect_peers(pair: &mut supervised::Pair) -> Result<(), Box<dyn Error>> {
    let (a, b) = (pair.a.cid, pair.b.cid);
    common::send(&mut pair.a.sink, peer_connect(a, b)).await?;
    expect(&mut pair.a.stream, |r| {
        matches!(r, InternalServiceResponse::PeerConnectSuccess(s) if s.peer_cid == b).then_some(())
    })
    .await
}

/// `from` sends `peer_cid` a chat message and waits for the peer's agent to acknowledge it.
async fn delivered_to(
    from: &mut supervised::Window,
    peer_cid: u64,
    content: &str,
) -> Result<(), Box<dyn Error>> {
    common::send(
        &mut from.sink,
        InternalServiceRequest::ConversationSend {
            request_id: Uuid::new_v4(),
            cid: from.cid,
            peer_cid,
            content: content.into(),
            message_type: MessageType::Text,
            reply_to: None,
            mentions: None,
            attachments: None,
            document_id: None,
            document_title: None,
            security_level: SecurityLevel::Standard,
        },
    )
    .await?;
    expect(&mut from.stream, |r| match r {
        InternalServiceResponse::ConversationEvent(e)
            if e.message.as_ref().is_some_and(|m| {
                matches!(m.status, MessageStatus::Delivered | MessageStatus::Read)
            }) =>
        {
            Some(())
        }
        _ => None,
    })
    .await
}

/// How long the agent has to find the peer again and the queue to drain: a reconnect and a
/// dial, not a keep-alive.
const REDIAL_BUDGET: Duration = Duration::from_secs(30);

#[tokio::test(flavor = "multi_thread")]
async fn a_severed_link_with_no_window_open_is_redialled_and_the_backlog_arrives(
) -> Result<(), Box<dyn Error>> {
    let mut pair = pair(Some(AGENT_SUPERVISOR), None).await?;
    connect_peers(&mut pair).await?;
    // Every window of `a` closes; its session stays signed in.
    let cid_a = pair.a.cid;
    drop(pair.a);
    pair.proxy.sever();
    tokio::time::timeout(
        REDIAL_BUDGET,
        delivered_to(&mut pair.b, cid_a, "while you were away"),
    )
    .await
    .map_err(|_| format!("nothing reached `a` within {REDIAL_BUDGET:?}: nobody redialled"))??;
    Ok(())
}

fn interest(session_cid: u64, peer_cid: u64, request_id: Uuid) -> InternalServiceRequest {
    InternalServiceRequest::ConnectionManagement {
        request_id,
        management_command: ConfigCommand::Interest {
            session_cid,
            peer_cid,
            until: unix_ms() + 60_000,
        },
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_open_chat_makes_the_agent_dial_for_it() -> Result<(), Box<dyn Error>> {
    let mut pair = pair(Some(AGENT_SUPERVISOR), None).await?;
    assert!(pair.a_caps.supervises_p2p, "a supervised agent says so");
    assert!(!pair.b_caps.supervises_p2p, "an unsupervised one does not");
    let (a, b) = (pair.a.cid, pair.b.cid);
    let request_id = Uuid::new_v4();
    common::send(&mut pair.a.sink, interest(a, b, request_id)).await?;
    expect(&mut pair.a.stream, |r| {
        matches!(&r, InternalServiceResponse::ConnectionManagementSuccess(s) if s.request_id == Some(request_id))
            .then_some(())
    })
    .await?;
    expect(&mut pair.a.stream, |r| {
        matches!(r, InternalServiceResponse::PeerConnectSuccess(s) if s.peer_cid == b).then_some(())
    })
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unsupervised_agent_refuses_interest_so_the_window_dials_itself(
) -> Result<(), Box<dyn Error>> {
    let mut pair = pair(None, None).await?;
    let (a, b) = (pair.a.cid, pair.b.cid);
    let request_id = Uuid::new_v4();
    common::send(&mut pair.a.sink, interest(a, b, request_id)).await?;
    expect(&mut pair.a.stream, |r| {
        matches!(&r, InternalServiceResponse::ConnectionManagementFailure(f) if f.request_id == Some(request_id))
            .then_some(())
    })
    .await
}

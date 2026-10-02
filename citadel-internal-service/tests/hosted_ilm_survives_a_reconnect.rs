//! An account whose ILM the agent hosts keeps it across a C2S drop the agent's own
//! reconnect recovers.
//!
//! Live on 0.8.6: the link dropped, the reconnect brought it back ("is back"), and every
//! send from the still-signed-in window failed with "session … has no agent-hosted ILM"
//! until a window declared again. The drop's teardown stopped the ILM, and nothing on
//! the way back started it.

// Shared with server_reconnect.rs, which uses the helpers this file does not.
#[allow(dead_code)]
#[path = "reconnect_support/mod.rs"]
mod reconnect;
#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service_connector::connector::WrappedSink;
use citadel_internal_service_connector::io_interface::tcp::TcpIOInterface;
use citadel_internal_service_test_common as common;
use citadel_internal_service_test_common::agent_ilm::declare_agent_ilm;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, MessageStatus, MessageType, SecurityLevel,
};
use citadel_sdk::prelude::*;
use reconnect::{next_link_event, Proxy};
use std::error::Error;
use support::{expect, open, register, spawn_agent, temp_store, username, Stream};
use uuid::Uuid;

/// A peer the account never connected to: the ILM queues for it, which is all a send
/// needs to be accepted.
const ABSENT_PEER: u64 = 4242;

async fn conversation_send(
    sink: &mut WrappedSink<TcpIOInterface>,
    stream: &mut Stream,
    cid: u64,
    content: &str,
) -> Result<InternalServiceResponse, Box<dyn Error>> {
    let request_id = Uuid::new_v4();
    common::send(
        sink,
        InternalServiceRequest::ConversationSend {
            request_id,
            cid,
            peer_cid: ABSENT_PEER,
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
    expect(stream, |response| {
        (response.request_id() == Some(&request_id)
            && matches!(
                response,
                InternalServiceResponse::ConversationUpdated(_)
                    | InternalServiceResponse::ConversationFailure(_)
            ))
        .then_some(response)
    })
    .await
}

fn sent(answer: &InternalServiceResponse) -> bool {
    matches!(
        answer,
        InternalServiceResponse::ConversationUpdated(u)
            if u.message.as_ref().is_some_and(|m| m.status == MessageStatus::Sent)
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hosted_account_still_sends_after_its_link_comes_back() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let (server, server_addr) = common::server_info_skip_cert_verification::<StackedRatchet>();
    tokio::spawn(server);
    let proxy = Proxy::start(server_addr).await?;
    let store = temp_store();
    let (agent, _node) = spawn_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;

    common::send(&mut sink, declare_agent_ilm()).await?;
    expect(&mut stream, |r| {
        matches!(r, InternalServiceResponse::AgentCapabilities(_)).then_some(())
    })
    .await?;
    let cid = register(&mut sink, &mut stream, &proxy.addr.to_string(), &username()).await??;

    let before = conversation_send(&mut sink, &mut stream, cid, "before the drop").await?;
    assert!(sent(&before), "hosted before the drop: {before:?}");

    proxy.sever();
    assert_eq!(
        next_link_event(&mut stream, cid).await?,
        "lost(reconnecting=true)"
    );
    assert_eq!(next_link_event(&mut stream, cid).await?, "reconnected");

    let after = conversation_send(&mut sink, &mut stream, cid, "after it is back").await?;
    assert!(
        sent(&after),
        "still hosted once the link is back: {after:?}"
    );
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

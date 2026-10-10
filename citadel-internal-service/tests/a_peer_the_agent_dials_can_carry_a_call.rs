//! A peer the agent dials can carry a call.
//!
//! The browser opens its server link with UDP off and dials peers with it on, because a call's
//! media needs a datagram path and nothing else does. A supervisor that dialled with the
//! server link's mode brought every peer up with no UDP path, and every call over a link the
//! agent dialled failed with "no usable UDP path".

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
use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
use std::error::Error;
use supervised::{interest, pair};
use support::expect;
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread")]
async fn a_peer_the_agent_dials_can_carry_a_call() -> Result<(), Box<dyn Error>> {
    let mut pair = pair(Some(AGENT_SUPERVISOR), None).await?;
    let (a, b) = (pair.a.cid, pair.b.cid);
    common::send(&mut pair.a.sink, interest(a, b, Uuid::new_v4())).await?;
    expect(&mut pair.a.stream, |r| {
        matches!(r, InternalServiceResponse::PeerConnectSuccess(s) if s.peer_cid == b).then_some(())
    })
    .await?;
    let request_id = Uuid::new_v4();
    let media_open = InternalServiceRequest::MediaOpen {
        request_id,
        cid: a,
        peer_cid: b,
    };
    common::send(&mut pair.a.sink, media_open).await?;
    let opened = expect(&mut pair.a.stream, |r| match r {
        InternalServiceResponse::MediaSessionOpened(o) if o.request_id == Some(request_id) => {
            Some(Ok(o.unreliable))
        }
        InternalServiceResponse::MediaSessionFailed(f) if f.request_id == Some(request_id) => {
            Some(Err(f.message))
        }
        _ => None,
    })
    .await?;
    match opened {
        Ok(true) => Ok(()),
        Ok(false) => Err("the call opened with no datagram path over the agent's dial".into()),
        Err(why) => Err(format!("the call could not open over the agent's dial: {why}").into()),
    }
}

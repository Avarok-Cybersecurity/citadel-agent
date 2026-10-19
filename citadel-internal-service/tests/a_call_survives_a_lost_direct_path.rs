//! A call whose direct path is lost keeps its media once the path is recovered.
//!
//! The supervisor asks the SDK to climb a relayed peer off the relay with its UDP channel restored
//! (`PathControl::upgrade(true)`), and the agent moves the live call to the restored channel
//! (`offer_restored_udp` -> `rebind_transport`). The move drops the old channel's receive half,
//! and with the SDK at the time that drop sent `DisconnectUDP` for the whole connection: the
//! restored channel was closed on both peers the moment the call moved to it, so a call that
//! survived a path loss went silent and stayed silent.
//!
//! Here two agents connect over UDP, open a call, and see a frame cross. Every UDP socket that
//! came up with the peer connection is then killed in place (both agents run in this process),
//! the path falls back to the relay, the supervisor recovers it, and a frame must cross again.
#![cfg(unix)]

#[allow(dead_code)]
#[path = "reconnect_support/mod.rs"]
mod reconnect;
#[allow(dead_code)]
#[path = "supervisor_support/mod.rs"]
mod supervised;
#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;
#[path = "udp_sockets_support/mod.rs"]
mod udp_sockets;

use citadel_internal_service::AGENT_SUPERVISOR;
use citadel_internal_service_test_common as common;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, P2pPathReport,
};
use futures::StreamExt;
use std::error::Error;
use std::time::Duration;
use supervised::{interest, Window};
use support::expect;
use uuid::Uuid;

/// Recovery waits out the supervisor's upgrade backoff and a fresh punch; generous for CI.
const RECOVERY: Duration = Duration::from_secs(90);

async fn open_call(window: &mut Window, peer_cid: u64) -> Result<(), Box<dyn Error>> {
    let request_id = Uuid::new_v4();
    let open = InternalServiceRequest::MediaOpen {
        request_id,
        cid: window.cid,
        peer_cid,
    };
    common::send(&mut window.sink, open).await?;
    let opened = expect(&mut window.stream, |r| match r {
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
        Ok(false) => Err("the call opened with no datagram path".into()),
        Err(why) => Err(format!("the call could not open: {why}").into()),
    }
}

/// A resends frames to B until one arrives, for up to `within`. Frames are datagrams. Each
/// phase's frames carry their own `mark`, so a frame still buffered from an earlier phase
/// cannot pass for one sent now.
async fn a_frame_crosses(a: &mut Window, b: &mut Window, mark: u8, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    let mut attempt = 0u32;
    while tokio::time::Instant::now() < deadline {
        attempt += 1;
        let frame = InternalServiceRequest::MediaSend {
            request_id: Uuid::new_v4(),
            cid: a.cid,
            peer_cid: b.cid,
            track: 0,
            kind: 0,
            timestamp: attempt,
            flags: 1,
            payload: vec![mark; 900],
        };
        if common::send(&mut a.sink, frame).await.is_err() {
            return false;
        }
        let wait = tokio::time::timeout(Duration::from_millis(200), b.stream.next());
        if let Ok(Some(InternalServiceResponse::MediaFrameNotification(f))) = wait.await {
            if f.peer_cid == a.cid && f.payload.first() == Some(&mark) {
                return true;
            }
        }
    }
    false
}

/// Waits for the path to `peer_cid` to become `path`; `settled` also requires no upgrade to be
/// running (a fall-back to the relay is reported with its retry already scheduled).
async fn path_becomes(
    window: &mut Window,
    peer_cid: u64,
    path: P2pPathReport,
    settled: bool,
) -> Result<(), Box<dyn Error>> {
    // Not `expect`: the stream can be quiet for longer than its per-message timeout while the
    // dead route is detected (a QUIC idle timeout), and only the overall deadline matters.
    let wanted = |r: &InternalServiceResponse| {
        matches!(r, InternalServiceResponse::PeerPathChangedNotification(n)
            if n.peer_cid == peer_cid && n.path == path && !(settled && n.upgrading))
    };
    tokio::time::timeout(RECOVERY, async {
        while let Some(response) = window.stream.next().await {
            if wanted(&response) {
                return Ok(());
            }
        }
        Err("the agent closed the connection".into())
    })
    .await
    .map_err(|_| format!("the path to {peer_cid} never became {path:?}"))?
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_keeps_its_media_after_the_direct_path_is_recovered() -> Result<(), Box<dyn Error>> {
    let mut pair = supervised::pair(Some(AGENT_SUPERVISOR), None).await?;
    // Both agents are signed in; nothing has dialled the peer yet.
    let before = udp_sockets::bound();
    let (a, b) = (pair.a.cid, pair.b.cid);
    common::send(&mut pair.a.sink, interest(a, b, Uuid::new_v4())).await?;
    expect(&mut pair.a.stream, |r| {
        matches!(r, InternalServiceResponse::PeerConnectSuccess(s) if s.peer_cid == b).then_some(())
    })
    .await?;
    open_call(&mut pair.a, b).await?;
    open_call(&mut pair.b, a).await?;
    assert!(
        a_frame_crosses(&mut pair.a, &mut pair.b, 1, Duration::from_secs(20)).await,
        "no frame crossed before the path was lost"
    );

    // Every UDP socket that came up since the dial belongs to the peer connection.
    let killed = udp_sockets::kill_all_except(&before);
    assert!(killed > 0, "no peer-connection UDP socket to kill");
    eprintln!("[test] killed {killed} UDP socket(s); waiting for the relay");
    path_becomes(&mut pair.a, b, P2pPathReport::ServerRelay, false).await?;
    eprintln!("[test] on the relay; waiting for the recovery");
    path_becomes(&mut pair.a, b, P2pPathReport::Direct, true).await?;
    eprintln!("[test] recovered");

    assert!(
        a_frame_crosses(&mut pair.a, &mut pair.b, 2, Duration::from_secs(20)).await,
        "the call went silent after its path was recovered"
    );
    Ok(())
}

//! Shared by the TURN integration tests: two agents on one server, a PeerConnect pair with
//! each side's TURN config, and message/media probes over the resulting peer connection.

use crate::{
    connect_p2p_with_turn, get_free_port, register_p2p, services_connected_to_one_server,
    PeerReturnHandle,
};
use citadel_internal_service_types::{
    IceServer, InternalServiceRequest, InternalServiceResponse, P2pPathReport, PeerTurnConfig,
    TurnPolicy,
};
use citadel_sdk::prelude::*;
use std::error::Error;
use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

pub const STEP: Duration = Duration::from_secs(30);

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

pub fn config(
    policy: TurnPolicy,
    urls: Vec<String>,
    user: &str,
    pass: &str,
    expires_at: u64,
) -> PeerTurnConfig {
    PeerTurnConfig {
        policy,
        ice_servers: vec![IceServer {
            urls,
            username: Some(user.to_string()),
            credential: Some(pass.to_string()),
        }],
        expires_at,
    }
}

/// A TURN URL nothing listens on: relaying through it can never succeed.
pub fn dead_turn_url() -> String {
    let dead = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    format!("turn:{}?transport=udp", dead.local_addr().unwrap())
}

pub async fn two_registered_agents() -> Result<Vec<PeerReturnHandle>, Box<dyn Error>> {
    let addrs: Vec<SocketAddr> = (0..2)
        .map(|_| format!("127.0.0.1:{}", get_free_port()).parse().unwrap())
        .collect();
    let mut agents = services_connected_to_one_server::<StackedRatchet>(addrs, None).await?;
    let (a, b) = agents.split_at_mut(1);
    let ((tx_a, rx_a, cid_a), (tx_b, rx_b, cid_b)) = (&mut a[0], &mut b[0]);
    register_p2p(
        tx_a,
        rx_a,
        *cid_a,
        tx_b,
        rx_b,
        *cid_b,
        SessionSecuritySettings::default(),
        None,
    )
    .await?;
    Ok(agents)
}

/// Brings the pair up with `turn` on both sides; returns both reported paths.
pub async fn connect(
    agents: &mut [PeerReturnHandle],
    turn: [Option<PeerTurnConfig>; 2],
) -> Result<[P2pPathReport; 2], Box<dyn Error>> {
    let (a, b) = agents.split_at_mut(1);
    let ((tx_a, rx_a, cid_a), (tx_b, rx_b, cid_b)) = (&mut a[0], &mut b[0]);
    tokio::time::timeout(
        Duration::from_secs(60),
        connect_p2p_with_turn(
            tx_a,
            rx_a,
            *cid_a,
            tx_b,
            rx_b,
            *cid_b,
            SessionSecuritySettings::default(),
            None,
            UdpMode::Enabled,
            turn,
        ),
    )
    .await
    .expect("PeerConnect did not settle within 60s")
}

pub async fn next_matching<T>(
    rx: &mut UnboundedReceiver<InternalServiceResponse>,
    what: &str,
    mut pick: impl FnMut(InternalServiceResponse) -> Option<T>,
) -> T {
    let deadline = tokio::time::Instant::now() + STEP;
    loop {
        let got = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .unwrap_or_else(|_| panic!("no {what} within {STEP:?}"))
            .expect("service channel closed");
        if let Some(v) = pick(got) {
            return v;
        }
    }
}

pub async fn message_one_way(
    tx: &UnboundedSender<InternalServiceRequest>,
    cid: u64,
    rx_peer: &mut UnboundedReceiver<InternalServiceResponse>,
    peer_cid: u64,
) {
    let body = format!("hello from {cid}").into_bytes();
    tx.send(InternalServiceRequest::Message {
        request_id: Uuid::new_v4(),
        message: body.clone(),
        cid,
        peer_cid: Some(peer_cid),
        security_level: Default::default(),
    })
    .unwrap();
    let got = next_matching(rx_peer, "MessageNotification", |r| match r {
        InternalServiceResponse::MessageNotification(n) if n.peer_cid == cid => Some(n),
        _ => None,
    })
    .await;
    assert_eq!(got.cid, peer_cid);
    assert_eq!(got.message, body);
}

pub async fn message_each_way(agents: &mut [PeerReturnHandle]) {
    let (a, b) = agents.split_at_mut(1);
    let ((tx_a, rx_a, cid_a), (tx_b, rx_b, cid_b)) = (&mut a[0], &mut b[0]);
    message_one_way(tx_a, *cid_a, rx_b, *cid_b).await;
    message_one_way(tx_b, *cid_b, rx_a, *cid_a).await;
}

pub async fn open_media(
    tx: &UnboundedSender<InternalServiceRequest>,
    rx: &mut UnboundedReceiver<InternalServiceResponse>,
    cid: u64,
    peer_cid: u64,
) {
    tx.send(InternalServiceRequest::MediaOpen {
        request_id: Uuid::new_v4(),
        cid,
        peer_cid,
    })
    .unwrap();
    let opened = next_matching(rx, "media session result", |r| match r {
        InternalServiceResponse::MediaSessionOpened(o) => Some(o),
        InternalServiceResponse::MediaSessionFailed(f) => {
            panic!("media open failed for {cid}: {}", f.message)
        }
        _ => None,
    })
    .await;
    assert!(opened.unreliable, "{cid}: media has no datagram path");
}

/// Both sides open a media session; A's frames reach B. Frames are datagrams and may be
/// dropped, so A resends until one arrives.
pub async fn media_datagram_crosses(agents: &mut [PeerReturnHandle]) {
    let (a, b) = agents.split_at_mut(1);
    let ((tx_a, rx_a, cid_a), (tx_b, rx_b, cid_b)) = (&mut a[0], &mut b[0]);
    open_media(tx_a, rx_a, *cid_a, *cid_b).await;
    open_media(tx_b, rx_b, *cid_b, *cid_a).await;
    let payload = vec![0xC7u8; 900];
    for attempt in 0..100u32 {
        tx_a.send(InternalServiceRequest::MediaSend {
            request_id: Uuid::new_v4(),
            cid: *cid_a,
            peer_cid: *cid_b,
            track: 0,
            kind: 0,
            timestamp: attempt,
            flags: 1,
            payload: payload.clone(),
        })
        .unwrap();
        match tokio::time::timeout(Duration::from_millis(200), rx_b.recv()).await {
            Ok(Some(InternalServiceResponse::MediaFrameNotification(frame))) => {
                assert_eq!(frame.peer_cid, *cid_a);
                assert_eq!(frame.payload, payload);
                return;
            }
            Ok(Some(_)) | Err(_) => continue,
            Ok(None) => panic!("B's service channel closed"),
        }
    }
    panic!("no media frame crossed the peer connection");
}

/// What each side of an accepted connection reported.
#[derive(Debug)]
pub struct AcceptOutcome {
    /// The initiator's `PeerConnectSuccess.path`, or its `PeerConnectFailure` message.
    pub initiator: Result<P2pPathReport, String>,
    /// Whether the acceptor's `PeerConnectAccept` was answered with success.
    pub accept_delivered: bool,
    /// The acceptor's `PeerConnectSuccess.path` (sent when its channel is created), if any.
    pub acceptor: Option<P2pPathReport>,
}

/// The browser's shape: A sends PeerConnect with `turn_a`, B is notified and answers with
/// PeerConnectAccept carrying `turn_b`.
pub async fn connect_by_accept(
    agents: &mut [PeerReturnHandle],
    turn_a: Option<PeerTurnConfig>,
    turn_b: Option<PeerTurnConfig>,
) -> AcceptOutcome {
    let (a, b) = agents.split_at_mut(1);
    let ((tx_a, rx_a, cid_a), (tx_b, rx_b, cid_b)) = (&mut a[0], &mut b[0]);
    let (cid_a, cid_b) = (*cid_a, *cid_b);
    tx_a.send(InternalServiceRequest::PeerConnect {
        request_id: Uuid::new_v4(),
        cid: cid_a,
        peer_cid: cid_b,
        udp_mode: UdpMode::Enabled,
        session_security_settings: SessionSecuritySettings::default(),
        peer_session_password: None,
        turn: turn_a,
    })
    .unwrap();
    next_matching(rx_b, "PeerConnectNotification", |r| match r {
        InternalServiceResponse::PeerConnectNotification(n) if n.peer_cid == cid_a => Some(()),
        _ => None,
    })
    .await;
    tx_b.send(InternalServiceRequest::PeerConnectAccept {
        request_id: Uuid::new_v4(),
        cid: cid_b,
        peer_cid: cid_a,
        accept: true,
        udp_mode: UdpMode::Enabled,
        session_security_settings: SessionSecuritySettings::default(),
        peer_session_password: None,
        turn: turn_b,
    })
    .unwrap();

    // The agent bounds the connect at 30s and then answers with a failure.
    let initiator = tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            match rx_a.recv().await.expect("service channel closed") {
                InternalServiceResponse::PeerConnectSuccess(s) => return Ok(s.path),
                InternalServiceResponse::PeerConnectFailure(f) => return Err(f.message),
                _ => continue,
            }
        }
    })
    .await
    .expect("the initiator's PeerConnect was never answered");

    let mut outcome = AcceptOutcome {
        initiator,
        accept_delivered: false,
        acceptor: None,
    };
    // Bounded past the SDK's 30s hole-punch timeout: an acceptor whose direct attempt has no
    // counterpart reports only once that attempt gives up.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    while !(outcome.accept_delivered && outcome.acceptor.is_some()) {
        match tokio::time::timeout_at(deadline, rx_b.recv()).await {
            Ok(Some(InternalServiceResponse::PeerConnectAcceptSuccess(s))) => {
                assert!(s.accept);
                outcome.accept_delivered = true;
            }
            Ok(Some(InternalServiceResponse::PeerConnectSuccess(s))) => {
                assert_eq!(s.peer_cid, cid_a);
                outcome.acceptor = Some(s.path);
            }
            Ok(Some(_)) => continue,
            Ok(None) => panic!("service channel closed"),
            Err(_) => break,
        }
    }
    outcome
}

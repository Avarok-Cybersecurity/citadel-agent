//! Two identity agents' accounts peered P2P through the UI's request protocol: register, dial,
//! accept as the UI does (`PeerConnectAccept`), and what each end's connection says about the
//! other's identity key.

use super::{create_identity, register_identity, signed_in};
use super::{expect, open, ChallengeServer, Sink, Stream};
use async_trait::async_trait;
use citadel_internal_service::kernel::p2p_kex::PeerKeyStore;
use citadel_internal_service_test_common as common;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, PeerIdentityMark,
};
use parking_lot::Mutex;
use std::error::Error;
use uuid::Uuid;

pub struct End {
    pub sink: Sink,
    pub stream: Stream,
    pub cid: u64,
}

pub async fn identity_end(
    agent: std::net::SocketAddr,
    server: &ChallengeServer,
    handle: &str,
) -> Result<(End, String), Box<dyn Error>> {
    let (mut sink, mut stream) = open(agent).await?;
    let id_fp = create_identity(&mut sink, &mut stream).await?;
    let request_id = Uuid::new_v4();
    common::send(&mut sink, register_identity(request_id, server, handle)).await?;
    let cid = signed_in(&mut stream, request_id).await??.cid;
    Ok((End { sink, stream, cid }, id_fp))
}

pub fn grouped(id_fp: &str) -> String {
    let groups: Vec<&str> = (0..id_fp.len())
        .step_by(4)
        .map(|at| &id_fp[at..at + 4])
        .collect();
    groups.join("-")
}

pub async fn peer_register(a: &mut End, b: &mut End) -> Result<(), Box<dyn Error>> {
    for (from, to) in [(a.cid, b.cid), (b.cid, a.cid)] {
        let end = if from == a.cid { &mut *a } else { &mut *b };
        common::send(
            &mut end.sink,
            InternalServiceRequest::PeerRegister {
                request_id: Uuid::new_v4(),
                cid: from,
                peer_cid: to,
                session_security_settings: citadel_sdk::prelude::default_session_security_settings(
                ),
                connect_after_register: false,
                peer_session_password: None,
            },
        )
        .await?;
    }
    for end in [a, b] {
        expect(&mut end.stream, |r| {
            matches!(r, InternalServiceResponse::PeerRegisterSuccess(_)).then_some(())
        })
        .await?;
    }
    Ok(())
}

/// `cid` answers `peer_cid`'s dial as the UI does (`PeerConnectAccept`).
pub fn accept(cid: u64, peer_cid: u64) -> InternalServiceRequest {
    InternalServiceRequest::PeerConnectAccept {
        request_id: Uuid::new_v4(),
        cid,
        peer_cid,
        accept: true,
        udp_mode: Default::default(),
        session_security_settings: citadel_sdk::prelude::default_session_security_settings(),
        peer_session_password: None,
        turn: None,
    }
}

pub fn dial(cid: u64, peer_cid: u64) -> InternalServiceRequest {
    InternalServiceRequest::PeerConnect {
        request_id: Uuid::new_v4(),
        cid,
        peer_cid,
        udp_mode: Default::default(),
        session_security_settings: citadel_sdk::prelude::default_session_security_settings(),
        peer_session_password: None,
        turn: None,
    }
}

/// `a` dials `b`, `b` accepts; each end's outcome: the mark, or the failure.
pub async fn connect(
    a: &mut End,
    b: &mut End,
) -> Result<[Result<PeerIdentityMark, String>; 2], Box<dyn Error>> {
    common::send(&mut a.sink, dial(a.cid, b.cid)).await?;
    expect(&mut b.stream, |r| {
        matches!(r, InternalServiceResponse::PeerConnectNotification(_)).then_some(())
    })
    .await?;
    common::send(&mut b.sink, accept(b.cid, a.cid)).await?;
    let outcome = |r: InternalServiceResponse| match r {
        InternalServiceResponse::PeerConnectSuccess(s) => Some(Ok(s.peer_identity)),
        InternalServiceResponse::PeerConnectFailure(f) => Some(Err(f.message)),
        _ => None,
    };
    let at_a = expect(&mut a.stream, outcome).await?;
    let at_b = match at_a {
        Ok(_) => expect(&mut b.stream, outcome).await?,
        Err(_) => Err("not reached".to_string()),
    };
    Ok([at_a, at_b])
}

pub fn verified(fingerprint: &str) -> Result<PeerIdentityMark, String> {
    Ok(PeerIdentityMark::Verified {
        fingerprint: fingerprint.to_string(),
    })
}

/// A store that already pinned `key` for the pair (alice's account, bob's).
pub struct PinnedBefore(pub Mutex<Option<Vec<u8>>>);

#[async_trait]
impl PeerKeyStore for PinnedBefore {
    async fn load(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(self.0.lock().clone())
    }
    async fn save(&self, book: Vec<u8>) -> Result<(), String> {
        *self.0.lock() = Some(book);
        Ok(())
    }
}

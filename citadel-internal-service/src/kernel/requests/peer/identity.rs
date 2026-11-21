//! `GetPeerIdentity` and `AcceptPeerIdentityKey` (kernel/p2p_kex): both ends' fingerprints for
//! the safety-number view, and the person's acceptance of a peer's changed key. Accepting is
//! only ever this request: the agent never re-pins a key on its own.

use crate::kernel::p2p_kex::grouped_fingerprint_of;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, PeerIdentityFailure, PeerIdentityInfo,
    PeerIdentityKeyAccepted,
};
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

pub(crate) fn handle<T, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let response = match request {
        InternalServiceRequest::GetPeerIdentity {
            request_id,
            cid,
            peer_cid,
        } => view(this, cid, peer_cid, request_id),
        InternalServiceRequest::AcceptPeerIdentityKey {
            request_id,
            cid,
            peer_cid,
            fingerprint,
        } => accept(this, cid, peer_cid, &fingerprint, request_id),
        _ => return None,
    };
    Some(HandledRequestResult { response, uuid })
}

fn failure(cid: u64, peer_cid: u64, message: String, request_id: Uuid) -> InternalServiceResponse {
    InternalServiceResponse::PeerIdentityFailure(PeerIdentityFailure {
        cid,
        peer_cid,
        message,
        request_id: Some(request_id),
    })
}

const NO_PINS: &str =
    "this agent keeps no peer identity keys (CitadelWorkspaceService::p2p_kex_identity)";

fn view<T, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    peer_cid: u64,
    request_id: Uuid,
) -> InternalServiceResponse {
    let Some(pins) = this.p2p_kex_pins.get() else {
        return failure(cid, peer_cid, NO_PINS.to_string(), request_id);
    };
    let own_fingerprint = this
        .server_connection_map
        .read()
        .get(&cid)
        .and_then(|session| session.p2p_kex_signer.clone())
        .map(|signer| citadel_identity::fingerprint_grouped(signer.public_key()));
    let (pinned, pending) = pins.view(cid, peer_cid);
    InternalServiceResponse::PeerIdentity(PeerIdentityInfo {
        cid,
        peer_cid,
        own_fingerprint,
        peer_fingerprint: pinned.as_deref().and_then(grouped_fingerprint_of),
        pending_fingerprint: pending.as_deref().and_then(grouped_fingerprint_of),
        request_id: Some(request_id),
    })
}

fn accept<T, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    peer_cid: u64,
    fingerprint: &str,
    request_id: Uuid,
) -> InternalServiceResponse {
    let Some(pins) = this.p2p_kex_pins.get() else {
        return failure(cid, peer_cid, NO_PINS.to_string(), request_id);
    };
    match pins.accept(cid, peer_cid, fingerprint) {
        Ok(key) => {
            citadel_sdk::logging::info!(target: "citadel", "[P2pKex] {cid}: the person accepted peer {peer_cid}'s new identity key");
            InternalServiceResponse::PeerIdentityKeyAccepted(PeerIdentityKeyAccepted {
                cid,
                peer_cid,
                fingerprint: grouped_fingerprint_of(&key).unwrap_or_default(),
                request_id: Some(request_id),
            })
        }
        Err(not) => failure(cid, peer_cid, not.to_string(), request_id),
    }
}

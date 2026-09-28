//! `EnableAgentIlm` and `SendReliable`: the requests by which a session uses the
//! agent-hosted ILM. Both have passed the ownership gate before they get here
//! (requests/mod.rs), which also refuses them for an unmapped session.
//!
//! The decisions are in kernel/ilm/service.rs; this supplies the production
//! store, peer links and delivery route, and turns outcomes into responses.
use crate::kernel::ilm::kv::SharedDb;
use crate::kernel::ilm::service::{EnableError, Enabled, SendReliableError};
use crate::kernel::ilm::transport::SessionPeerLinks;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::session_route::SessionRoute;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    EnableAgentIlmFailure, EnableAgentIlmSuccess, InternalServiceRequest, InternalServiceResponse,
    SendReliableFailure, SendReliableSuccess,
};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::Ratchet;
use std::sync::Arc;
use uuid::Uuid;

pub async fn handle_enable<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::EnableAgentIlm { request_id, cid } = request else {
        unreachable!("Should never happen if programmed properly")
    };

    let route = {
        let map = this.server_connection_map.read();
        map.get(&cid).map(|conn| {
            SessionRoute::new(
                conn.associated_localhost_connection.clone(),
                this.tx_to_localhost_clients.clone(),
            )
        })
    };
    let outcome = match route {
        // Removed between the gate and here.
        None => Err(EnableFailure::NoSession),
        Some(route) => {
            let db: SharedDb = Arc::new(this.remote().clone());
            let links = SessionPeerLinks::new(this.server_connection_map.clone());
            this.agent_ilm
                .enable(cid, db, links, route)
                .await
                .map_err(EnableFailure::Refused)
        }
    };
    match &outcome {
        Ok(enabled) => info!(target: "citadel", "[AGENT-ILM] {cid} opted in: {enabled:?}"),
        Err(failure) => warn!(target: "citadel", "[AGENT-ILM] {cid} opt-in refused: {failure:?}"),
    }
    Some(HandledRequestResult {
        response: enable_response(cid, request_id, outcome),
        uuid,
    })
}

#[derive(Debug)]
pub(crate) enum EnableFailure {
    NoSession,
    Refused(EnableError),
}

pub(crate) fn enable_response(
    cid: u64,
    request_id: Uuid,
    outcome: Result<Enabled, EnableFailure>,
) -> InternalServiceResponse {
    let message = match outcome {
        Ok(enabled) => {
            return InternalServiceResponse::EnableAgentIlmSuccess(EnableAgentIlmSuccess {
                cid,
                already_hosted: enabled == Enabled::AlreadyHosted,
                request_id: Some(request_id),
            })
        }
        Err(EnableFailure::NoSession) => format!("Connection for {cid} not found"),
        Err(EnableFailure::Refused(EnableError::NotOffered)) => {
            "This agent does not offer agent-hosted ILM".to_string()
        }
        Err(EnableFailure::Refused(EnableError::StillStarting)) => {
            "An opt-in for this account is still starting; retry".to_string()
        }
        Err(EnableFailure::Refused(EnableError::StoppedWhileStarting)) => {
            "The session ended while its ILM was starting".to_string()
        }
        Err(EnableFailure::Refused(EnableError::Backend(reason))) => {
            format!("Failed to load this account's ILM state: {reason}")
        }
    };
    InternalServiceResponse::EnableAgentIlmFailure(EnableAgentIlmFailure {
        cid,
        message,
        request_id: Some(request_id),
    })
}

pub async fn handle_send_reliable<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::SendReliable {
        request_id,
        cid,
        peer_cid,
        message,
        security_level,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };
    let outcome = this
        .agent_ilm
        .send_reliable(request_id, cid, peer_cid, message, security_level)
        .await;
    if let Err(err) = &outcome {
        warn!(target: "citadel", "[AGENT-ILM] SendReliable from {cid} to {peer_cid} refused: {err:?}");
    }
    Some(HandledRequestResult {
        response: send_reliable_response(cid, peer_cid, request_id, outcome),
        uuid,
    })
}

pub(crate) fn send_reliable_response(
    cid: u64,
    peer_cid: u64,
    request_id: Uuid,
    outcome: Result<(), SendReliableError>,
) -> InternalServiceResponse {
    let message = match outcome {
        Ok(()) => {
            return InternalServiceResponse::SendReliableSuccess(SendReliableSuccess {
                cid,
                peer_cid,
                request_id: Some(request_id),
            })
        }
        Err(SendReliableError::NotOptedIn) => {
            "This session has not opted in to agent-hosted ILM".to_string()
        }
        Err(SendReliableError::Ilm(reason)) => format!("Agent-hosted ILM refused it: {reason}"),
    };
    InternalServiceResponse::SendReliableFailure(SendReliableFailure {
        cid,
        peer_cid,
        message,
        request_id: Some(request_id),
    })
}

#[cfg(test)]
mod tests {
    //! Only a session's owner may opt it in or speak through its ILM; anyone
    //! else is refused with an answer, not silence. The real gate and refusal.
    use super::super::{gate_decision, refusal_response, GateDecision};
    use citadel_internal_service_types::{
        InternalServiceRequest, InternalServiceResponse, SecurityLevel,
    };
    use uuid::Uuid;

    fn requests() -> [InternalServiceRequest; 2] {
        [
            InternalServiceRequest::EnableAgentIlm {
                request_id: Uuid::from_u128(1),
                cid: 7,
            },
            InternalServiceRequest::SendReliable {
                request_id: Uuid::from_u128(2),
                cid: 7,
                peer_cid: 9,
                message: b"x".to_vec(),
                security_level: SecurityLevel::Standard,
            },
        ]
    }

    #[test]
    fn only_the_owner_passes_the_gate() {
        let (owner, stranger) = (Uuid::from_u128(10), Uuid::from_u128(11));
        for request in requests() {
            assert!(matches!(
                gate_decision(&request, Some(owner), owner),
                GateDecision::Proceed
            ));
            for held_by in [Some(owner), None] {
                assert!(
                    matches!(
                        gate_decision(&request, held_by, stranger),
                        GateDecision::Refuse { .. }
                    ),
                    "{request:?} held by {held_by:?} passed for a stranger"
                );
            }
        }
    }

    #[test]
    fn a_refusal_is_answered_with_the_requests_own_failure() {
        let caller = Uuid::from_u128(11);
        for request in requests() {
            let refused = refusal_response(&request, caller).expect("refused in silence");
            assert_eq!(refused.uuid, caller);
            match refused.response {
                InternalServiceResponse::EnableAgentIlmFailure(f) => {
                    assert_eq!((f.cid, f.request_id), (7, Some(Uuid::from_u128(1))))
                }
                InternalServiceResponse::SendReliableFailure(f) => assert_eq!(
                    (f.cid, f.peer_cid, f.request_id),
                    (7, 9, Some(Uuid::from_u128(2)))
                ),
                other => panic!("wrong answer to {request:?}: {other:?}"),
            }
        }
    }
}

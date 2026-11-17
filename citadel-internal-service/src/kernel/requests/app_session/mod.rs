//! `AppSessionOpen`, `AppSessionSend` and `AppSessionClose` (kernel/apps/session): the caller
//! must be attached to the member session it names and act as its user; the workspace server's
//! grant decides the rest, before the agent dials anything.

mod transport;

use crate::kernel::apps::session::core;
use crate::kernel::apps::session::rules::Refusal;
use crate::kernel::apps::session::Owner;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::server_link::ServerLinks;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    AppSessionFailure, AppSessionState, InternalServiceRequest, InternalServiceResponse,
    SessionStatus,
};
use citadel_sdk::logging::info;
use citadel_sdk::prelude::Ratchet;
use transport::ServiceTransport;
use uuid::Uuid;

/// The `AppSessionClose` answer's reason.
const CLOSED: &str = "closed";

pub(crate) fn failure(
    cid: u64,
    request_id: Uuid,
    install_id: String,
    refusal: &Refusal,
) -> InternalServiceResponse {
    if let Refusal::Unavailable(why) = refusal {
        info!(target: "citadel", "[APP-SESSION] {cid} {install_id}: {why}");
    }
    InternalServiceResponse::AppSessionFailure(AppSessionFailure {
        cid,
        request_id: Some(request_id),
        install_id,
        reason: refusal.wire().to_string(),
    })
}

/// Whether `uuid` is attached to member session `cid` and that session is `acting_username`'s.
fn acts_for<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    cid: u64,
    acting_username: &str,
) -> bool {
    let map = this.server_connection_map.read();
    map.get(&cid)
        .is_some_and(|conn| conn.subscribers.contains(uuid) && conn.username == acting_username)
}

pub(crate) async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let transport = ServiceTransport { this };
    let sessions = &this.apps.sessions;
    let response = match request {
        InternalServiceRequest::AppSessionOpen {
            request_id,
            cid,
            app_id,
            install_id,
            acting_username,
            server,
        } => {
            let fail = |refusal: Refusal| failure(cid, request_id, install_id.clone(), &refusal);
            if !acts_for(this, uuid, cid, &acting_username) {
                return answer(uuid, fail(Refusal::Session));
            }
            let Some(net) = this.apps.net.clone() else {
                return answer(uuid, fail(Refusal::NotConfigured));
            };
            let link = ServerLinks::new(this.server_connection_map.clone());
            let grant = net.grants.resolve(cid, &install_id, &link).await;
            let owner = Owner {
                member_cid: cid,
                install_id: install_id.clone(),
                acting_username,
            };
            match core::open(sessions, &transport, grant, &app_id, owner, &server).await {
                Ok((session, server_cid, state)) => {
                    state_of(cid, request_id, install_id, session, server_cid, state)
                }
                Err(refusal) => fail(refusal),
            }
        }
        InternalServiceRequest::AppSessionSend {
            request_id,
            cid,
            install_id,
            session,
            body,
        } => {
            if !this.attached(cid, uuid) {
                return answer(
                    uuid,
                    failure(cid, request_id, install_id, &Refusal::Session),
                );
            }
            match core::send(sessions, &transport, cid, &install_id, session, body).await {
                Ok(()) => return None,
                Err(refusal) => failure(cid, request_id, install_id, &refusal),
            }
        }
        InternalServiceRequest::AppSessionClose {
            request_id,
            cid,
            install_id,
            session,
        } => {
            if !this.attached(cid, uuid) {
                return answer(
                    uuid,
                    failure(cid, request_id, install_id, &Refusal::Session),
                );
            }
            match core::close(sessions, &transport, cid, &install_id, session).await {
                Ok(()) => {
                    let closed = SessionStatus::Closed {
                        reason: CLOSED.to_string(),
                    };
                    state_of(cid, request_id, install_id, session, 0, closed)
                }
                Err(refusal) => failure(cid, request_id, install_id, &refusal),
            }
        }
        _ => unreachable!("dispatched by variant"),
    };
    answer(uuid, response)
}

fn answer(uuid: Uuid, response: InternalServiceResponse) -> Option<HandledRequestResult> {
    Some(HandledRequestResult { response, uuid })
}

fn state_of(
    cid: u64,
    request_id: Uuid,
    install_id: String,
    session: u32,
    server_cid: u64,
    state: SessionStatus,
) -> InternalServiceResponse {
    InternalServiceResponse::AppSessionState(AppSessionState {
        cid,
        request_id: Some(request_id),
        install_id,
        session,
        server_cid,
        state,
    })
}

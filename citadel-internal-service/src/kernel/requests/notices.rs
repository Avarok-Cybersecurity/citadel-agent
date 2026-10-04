//! The notice plane's requests (kernel/notices): the native app subscribing
//! and muting with its launch token, and a window saying what it shows.

use crate::kernel::requests::connection_management::refusal;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::supervisor::Signal;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    ConfigCommand, ConnectionManagementSuccess, InternalServiceRequest, InternalServiceResponse,
    NoticeFailure, NoticeRows,
};
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

/// The same answer for no token configured and the wrong token: which one it
/// was would tell a guesser whether to keep guessing.
const REFUSED: &str = "The notice plane is not open to this connection";

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let (request_id, token) = match &request {
        InternalServiceRequest::NoticeSubscribe { request_id, token }
        | InternalServiceRequest::NoticeSetMuted {
            request_id, token, ..
        } => (*request_id, token),
        _ => return None,
    };
    if !this.notices.admits(token) {
        warn!(target: "citadel", "[NOTICES] connection {uuid} refused the notice plane");
        let response = InternalServiceResponse::NoticeFailure(NoticeFailure {
            cid: 0,
            message: REFUSED.to_string(),
            request_id: Some(request_id),
        });
        return Some(HandledRequestResult { response, uuid });
    }
    match request {
        InternalServiceRequest::NoticeSubscribe { .. } => {
            this.notices.subscribe(uuid);
            this.watch_notice_subscriber(uuid);
        }
        InternalServiceRequest::NoticeSetMuted { cid, muted, .. } => {
            if let Err(e) = this.set_muted(cid, muted).await {
                let response = InternalServiceResponse::NoticeFailure(NoticeFailure {
                    cid,
                    message: format!("The mute was not saved: {e}"),
                    request_id: Some(request_id),
                });
                return Some(HandledRequestResult { response, uuid });
            }
            this.rows_changed();
        }
        _ => return None,
    }
    let rows = NoticeRows {
        cid: 0,
        rows: this.notice_rows().await,
        request_id: Some(request_id),
    };
    Some(HandledRequestResult {
        response: InternalServiceResponse::NoticeRows(rows),
        uuid,
    })
}

/// A window says it has `cid` (and `peer`'s conversation) in front of the
/// user, or no longer does. Only a window attached to the session may say so.
pub(crate) fn report_focus<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    conn_id: Uuid,
    request_id: Uuid,
    command: ConfigCommand,
) -> Option<HandledRequestResult> {
    let ConfigCommand::ReportFocus {
        session_cid: cid,
        peer_cid: peer,
        focused,
    } = command
    else {
        return None;
    };
    let attached =
        crate::kernel::membership::subscribers_of(this, cid).is_some_and(|s| s.contains(conn_id));
    if !attached {
        return Some(refusal(
            cid,
            request_id,
            conn_id,
            format!("Session {cid} is not attached to this connection"),
        ));
    }
    this.notices.report_focus(conn_id, cid, peer, focused);
    let response =
        InternalServiceResponse::ConnectionManagementSuccess(ConnectionManagementSuccess {
            cid,
            request_id: Some(request_id),
            message: "Focus noted".to_string(),
        });
    Some(HandledRequestResult {
        response,
        uuid: conn_id,
    })
}

/// A window has `peer` open until `until` (Unix ms): the supervisor keeps it connected.
/// Refused when no supervisor runs for the account, which tells the window to dial itself.
pub(crate) fn declare_p2p_interest<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    conn_id: Uuid,
    request_id: Uuid,
    command: ConfigCommand,
) -> Option<HandledRequestResult> {
    let ConfigCommand::Interest {
        session_cid: cid,
        peer_cid,
        until,
    } = command
    else {
        return None;
    };
    let attached =
        crate::kernel::membership::subscribers_of(this, cid).is_some_and(|s| s.contains(conn_id));
    if !attached {
        return Some(refusal(
            cid,
            request_id,
            conn_id,
            format!("Session {cid} is not attached to this connection"),
        ));
    }
    if !this.supervisors.is_running(cid) {
        return Some(refusal(
            cid,
            request_id,
            conn_id,
            format!("Session {cid}'s peers are not supervised by this agent"),
        ));
    }
    this.supervisors.signal(
        cid,
        Signal::Interest {
            peer: peer_cid,
            ttl: std::time::Duration::from_millis(until.saturating_sub(unix_ms())),
        },
    );
    Some(HandledRequestResult {
        response: InternalServiceResponse::ConnectionManagementSuccess(
            ConnectionManagementSuccess {
                cid,
                request_id: Some(request_id),
                message: "Interest noted".to_string(),
            },
        ),
        uuid: conn_id,
    })
}

/// The wall clock, at the request edge: the supervisor's own time is monotonic and relative.
fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    GetSessionsResponse, InternalServiceRequest, InternalServiceResponse, PeerSessionInformation,
    SessionInformation,
};
use citadel_sdk::logging::info;
use citadel_sdk::prelude::{Ratchet, TargetLockedRemote};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::GetSessions { request_id } = request else {
        unreachable!("Should never happen if programmed properly")
    };
    let server_connection_map = &this.server_connection_map;
    let lock = server_connection_map.read();
    let mut sessions = Vec::new();

    info!(target: "citadel", "GetSessions: Found {} total sessions in server_connection_map", lock.len());

    // MODIFIED: Get ALL sessions, not just ones for current connection
    // This allows us to see orphaned sessions from other connections
    for (cid, connection) in lock.iter() {
        let conn_id = connection.associated_tcp_connection.load(Ordering::Relaxed);
        info!(target: "citadel", "GetSessions: Session {} for user {} associated with connection {}", cid, connection.username, conn_id);
        // Don't filter by current connection uuid - return all sessions
        let mut session = SessionInformation {
            cid: *cid,
            username: connection.username.clone(),
            peer_connections: HashMap::new(),
        };
        for (peer_cid, conn) in connection.peers.iter() {
            session.peer_connections.insert(
                *peer_cid,
                PeerSessionInformation {
                    cid: *cid,
                    peer_cid: *peer_cid,
                    peer_username: conn
                        .remote
                        .as_ref()
                        .and_then(|r| r.target_username())
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                },
            );
        }
        sessions.push(session);
    }

    let response = InternalServiceResponse::GetSessionsResponse(GetSessionsResponse {
        cid: 0,
        sessions,
        request_id: Some(request_id),
    });

    Some(HandledRequestResult { response, uuid })
}

use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, PeerDisconnectFailure, PeerDisconnectSuccess,
};
use citadel_sdk::prelude::{NodeRequest, PeerCommand, PeerConnectionType, PeerSignal, Ratchet};
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::PeerDisconnect {
        request_id,
        cid,
        peer_cid,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };
    let remote = this.remote();

    let disconnect_request = NodeRequest::PeerCommand(PeerCommand {
        session_cid: cid,
        command: PeerSignal::Disconnect {
            peer_conn_type: PeerConnectionType::LocalGroupPeer {
                session_cid: cid,
                peer_cid,
            },
            disconnect_response: None,
        },
    });

    // Check if connection and peer exist - extract error or proceed
    let can_disconnect: Result<(), String> = {
        let lock = this.server_connection_map.read();
        match lock.get(&cid) {
            None => Err("disconnect: Server connection not found".to_string()),
            Some(conn) => {
                if conn.peers.contains_key(&peer_cid) {
                    Ok(())
                } else {
                    Err(format!("Peer Connection {peer_cid} Not Found"))
                }
            }
        }
    }; // Lock dropped here - BEFORE any await

    let response = match can_disconnect {
        Err(message) => InternalServiceResponse::PeerDisconnectFailure(PeerDisconnectFailure {
            cid,
            message,
            request_id: Some(request_id),
        }),
        Ok(()) => {
            match remote.send(disconnect_request).await {
                Ok(_) => {
                    this.server_connection_map
                        .write()
                        .get_mut(&cid)
                        .map(|r| r.clear_peer_connection(peer_cid));

                    InternalServiceResponse::PeerDisconnectSuccess(PeerDisconnectSuccess {
                        cid,
                        request_id: Some(request_id),
                    })
                }
                Err(err) => {
                    InternalServiceResponse::PeerDisconnectFailure(PeerDisconnectFailure {
                        cid,
                        message: format!("Failed to disconnect: {err:?}"),
                        request_id: Some(request_id),
                    })
                }
            }
        }
    };

    Some(HandledRequestResult { response, uuid })
}

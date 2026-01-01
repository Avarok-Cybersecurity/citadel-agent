use crate::kernel::requests::HandledRequestResult;
use crate::kernel::{AsyncSink, CitadelWorkspaceService};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, MessageSendFailure, MessageSendSuccess,
};
use citadel_sdk::logging::info;
use citadel_sdk::prelude::{Ratchet, SecurityLevel};
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::Message {
        request_id,
        message,
        cid,
        peer_cid,
        security_level,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };

    // Clone the sink Arc BEFORE dropping the lock - this is the async-safe pattern
    // that avoids holding the RwLock across await points
    let sink_result: Result<(AsyncSink<R>, SecurityLevel), String> = {
        let server_connection_map = this.server_connection_map.read();
        match server_connection_map.get(&cid) {
            Some(conn) => {
                if let Some(peer_cid) = peer_cid {
                    // send to peer
                    info!(target: "citadel", "[P2P-MSG] Sending message from {cid} to peer {peer_cid}");
                    info!(target: "citadel", "[P2P-MSG] Available peers in conn.peers: {:?}", conn.peers.keys().collect::<Vec<_>>());
                    if let Some(peer_conn) = conn.peers.get(&peer_cid) {
                        info!(target: "citadel", "[P2P-MSG] Found peer connection, cloning sink Arc");
                        Ok((peer_conn.sink.clone(), security_level))
                    } else {
                        citadel_sdk::logging::error!(target: "citadel","[P2P-MSG] Peer connection not found for peer_cid={peer_cid}");
                        Err(format!("Peer connection for {peer_cid} not found"))
                    }
                } else {
                    // send to server
                    info!(target: "citadel", "[P2P-MSG] Sending message from {cid} to SERVER (no peer_cid)");
                    Ok((conn.sink_to_server.clone(), security_level))
                }
            }
            None => {
                info!(target: "citadel", "connection not found");
                Err(format!("Connection for {cid} not found"))
            }
        }
    }; // RwLock dropped here - BEFORE any await

    match sink_result {
        Ok((sink, security_level)) => {
            // Now lock the sink's inner Mutex (can hold across await)
            let mut sink_guard = sink.lock().await;
            sink_guard.set_security_level(security_level);

            info!(target: "citadel", "[P2P-MSG] About to call sink.send() for message from {} to {:?}", cid, peer_cid);
            if let Err(err) = sink_guard.send(message).await {
                let response = InternalServiceResponse::MessageSendFailure(MessageSendFailure {
                    cid,
                    message: format!("Error sending message: {err:?}"),
                    request_id: Some(request_id),
                });
                Some(HandledRequestResult { response, uuid })
            } else {
                info!(target: "citadel", "[P2P-MSG] sink.send() SUCCEEDED for message from {} to {:?}", cid, peer_cid);
                let response = InternalServiceResponse::MessageSendSuccess(MessageSendSuccess {
                    cid,
                    peer_cid,
                    request_id: Some(request_id),
                });
                Some(HandledRequestResult { response, uuid })
            }
        }
        Err(error_msg) => {
            let response = InternalServiceResponse::MessageSendFailure(MessageSendFailure {
                cid,
                message: error_msg,
                request_id: Some(request_id),
            });
            Some(HandledRequestResult { response, uuid })
        }
    }
}

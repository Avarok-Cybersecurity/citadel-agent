//! PeerRegisterRespond Handler
//!
//! Handles responses to incoming peer registration requests.
//! When a user clicks "Accept" on a pending registration notification,
//! this handler retrieves the stored PostRegister signal and calls
//! `responses::peer_register()` to complete the handshake.
//!
//! This allows the initiator's `register_to_peer().await` to unblock and return.

use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, PeerRegisterFailure, PeerRegisterSuccess,
};
use citadel_sdk::logging::{error, info};
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::PeerRegisterRespond {
        request_id,
        cid,
        peer_cid,
        accept,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };

    info!(target: "citadel", "[PeerRegisterRespond] cid={}, peer_cid={}, accept={}", cid, peer_cid, accept);

    // Retrieve the stored signal
    let signal = {
        let mut signals = this.pending_peer_registrations.write();
        signals.remove(&(cid, peer_cid))
    };

    let Some(signal) = signal else {
        error!(target: "citadel", "[PeerRegisterRespond] No pending registration found for (cid={}, peer_cid={})", cid, peer_cid);
        return Some(HandledRequestResult {
            response: InternalServiceResponse::PeerRegisterFailure(PeerRegisterFailure {
                cid,
                message: format!("No pending registration found for peer {}", peer_cid),
                request_id: Some(request_id),
            }),
            uuid,
        });
    };

    info!(target: "citadel", "[PeerRegisterRespond] Retrieved stored signal, calling responses::peer_register()");

    // Call the SDK's responses::peer_register to complete the handshake
    let remote = this.remote();
    match citadel_sdk::responses::peer_register(signal, accept, remote).await {
        Ok(ticket) => {
            info!(target: "citadel", "[PeerRegisterRespond] SUCCESS, ticket={:?}", ticket);
            // Get peer username from cache
            let peer_username = this
                .peer_username_cache
                .read()
                .get(&(cid, peer_cid))
                .cloned()
                .unwrap_or_default();

            Some(HandledRequestResult {
                response: InternalServiceResponse::PeerRegisterSuccess(PeerRegisterSuccess {
                    cid,
                    peer_cid,
                    peer_username,
                    request_id: Some(request_id),
                }),
                uuid,
            })
        }
        Err(err) => {
            let err_str = err.into_string();
            error!(target: "citadel", "[PeerRegisterRespond] FAILED: {}", err_str);
            Some(HandledRequestResult {
                response: InternalServiceResponse::PeerRegisterFailure(PeerRegisterFailure {
                    cid,
                    message: err_str,
                    request_id: Some(request_id),
                }),
                uuid,
            })
        }
    }
}

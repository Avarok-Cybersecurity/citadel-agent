//! The SDK half of a claim: a session must be live in the SDK to be handed over --
//! unless the agent is reconnecting it, when there is no SDK session to find yet.
//!
//! Split out of `connection_management_claim.rs`, which was over the 250-line cap.

use crate::kernel::requests::HandledRequestResult;
use crate::kernel::{send_response_to_tcp_client, CitadelWorkspaceService};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::*;
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::*;
use uuid::Uuid;

/// The refusal for a session the SDK does not hold, which is also removed; None when it does.
pub(super) async fn refuse_unless_sdk_holds<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    conn_id: Uuid,
    request_id: Uuid,
    session_cid: u64,
) -> Option<HandledRequestResult> {
    // Step 3: Verify session is active in SDK before allowing claim
    let remote = this.remote();
    let sdk_active_cids: Vec<u64> = match remote.sessions().await {
        Ok(conns) => conns.sessions.into_iter().map(|s| s.cid).collect(),
        Err(e) => {
            // An empty list here is not "the SDK has no sessions", it is "we
            // could not ask". Step 4 treats absence as proof the session is
            // dead and REMOVES it from the map before denying the claim, so a
            // transient stream error destroyed a live, claimable session and
            // told the user it was not claimable.
            warn!(
                target: "citadel",
                "ClaimSession: Failed to query SDK sessions: {:?}; refusing rather than \
                 treating the session as dead",
                e
            );
            return Some(HandledRequestResult {
                response: InternalServiceResponse::ConnectionManagementFailure(
                    ConnectionManagementFailure {
                        cid: session_cid,
                        request_id: Some(request_id),
                        error: format!(
                            "Could not determine whether session {} is still active: {:?}. \
                             Nothing was changed; try again.",
                            session_cid, e
                        ),
                    },
                ),
                uuid: conn_id,
            });
        }
    };

    info!(target: "citadel", "ClaimSession: SDK reports {} active sessions: {:?}", sdk_active_cids.len(), sdk_active_cids);

    // Step 4: Check if session is active in SDK
    if !sdk_active_cids.contains(&session_cid) {
        // Session exists in internal service but not in SDK - clean up and deny
        {
            let mut server_connection_map = this.server_connection_map.write();
            server_connection_map.remove(&session_cid);
        }
        // The CID-keyed kernel maps outlive the entry otherwise — see
        // prune_cid_scoped_state. Every other teardown site prunes; this one and
        // the two in DisconnectOrphan did not, and the gate could not see them
        // because they bind the write guard to a local before removing.
        //
        // Outside the guard: prune takes its own locks, and every other caller
        // releases the map first.
        this.prune_cid_scoped_state(session_cid, None);
        info!(target: "citadel", "ClaimSession: Session {} removed - not active in SDK", session_cid);
        return Some(HandledRequestResult {
            response: InternalServiceResponse::ConnectionManagementFailure(
                ConnectionManagementFailure {
                    cid: session_cid,
                    request_id: Some(request_id),
                    error: format!(
                        "Session {} is not claimable: SDK session is disconnected",
                        session_cid
                    ),
                },
            ),
            uuid: conn_id,
        });
    }

    None
}

/// The claimer now owns a session whose server link is down and being brought back.
pub(super) fn tell_the_claimer_it_is_reconnecting<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    session_cid: u64,
    conn_id: Uuid,
) {
    info!(target: "citadel", "ClaimSession: {} is reconnecting to its server; claimed without an SDK session", session_cid);
    let notice = InternalServiceResponse::ServerConnectionLost(ServerConnectionLost {
        cid: session_cid,
        reconnecting: true,
        request_id: None,
    });
    if let Err(err) = send_response_to_tcp_client(&this.tx_to_localhost_clients, notice, conn_id) {
        warn!(target: "citadel", "ClaimSession: telling {conn_id} that {session_cid} is reconnecting failed: {err:?}");
    }
}

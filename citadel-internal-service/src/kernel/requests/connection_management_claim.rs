//! Claiming a session for this localhost connection.
//!
//! Split out of `connection_management.rs`: this arm is longer than the other
//! three put together, and the file was over the 250-line cap. The
//! authorization rule it enforces lives in `connection_management_auth.rs`,
//! shared with the other two session-mutating commands so the three cannot
//! drift apart.

use crate::kernel::reconnect::{policy, LinkState};
use crate::kernel::requests::connection_management::{owner_of, refusal};
use crate::kernel::requests::connection_management_auth::{may_claim, Authorization, SessionOwner};
use crate::kernel::requests::connection_management_claim_sdk::{
    refuse_unless_sdk_holds, tell_the_claimer_it_is_reconnecting,
};
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::*;
use citadel_sdk::logging::info;
use citadel_sdk::prelude::*;
use std::sync::atomic::Ordering;
use uuid::Uuid;

/// The whole claim decision — the `only_if_orphaned` requirement and the
/// ownership rule — made from one reading of the session's state.
///
/// This runs twice per claim, and the second run is the one that counts.
/// Requests are handled in parallel tasks (see the spawn in `kernel/mod.rs`),
/// and Step 3 below awaits an SDK round-trip with no lock held, so two
/// connections claiming the same orphan can both pass the early check during
/// each other's await. Whichever takes the write lock second must see the
/// first one's re-point and be refused — otherwise both callers are told
/// they own the session, and every CID-routed notification follows whichever
/// wrote last while the loser's tab listens to nothing. Hence the decision
/// is re-made in Step 5 on state read under the very write lock that
/// performs the re-point.
///
/// The messages are load-bearing: `claim-session.ts` matches "not orphaned"
/// (another tab has it) and `tests/session_takeover.rs` matches "in use by
/// another connection". The race's loser lands on "not orphaned" — exactly
/// what it would have been told had the two requests been serialized.
fn decide_claim(
    owner: SessionOwner,
    only_if_orphaned: bool,
    caller: Uuid,
    session_cid: u64,
) -> Result<(), String> {
    if only_if_orphaned && matches!(owner, SessionOwner::Live(_)) {
        return Err(format!("Session {} is not orphaned", session_cid));
    }
    match may_claim(owner, caller, session_cid) {
        Authorization::Allow => Ok(()),
        Authorization::Refuse(error) => Err(error),
    }
}

fn link_of<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
) -> Option<LinkState> {
    this.server_connection_map
        .read()
        .get(&cid)
        .map(|conn| conn.link)
}

pub(super) async fn claim_session<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    conn_id: Uuid,
    request_id: Uuid,
    session_cid: u64,
    only_if_orphaned: bool,
) -> Option<HandledRequestResult> {
    // Steps 1-2: cheap refusal before the SDK round-trip. Advisory only —
    // the state it reads is stale the moment the lock inside `owner_of` is
    // released, so Step 5 decides again on state it can trust.
    let Some(owner) = owner_of(this, session_cid) else {
        return Some(HandledRequestResult {
            response: InternalServiceResponse::ConnectionManagementFailure(
                ConnectionManagementFailure {
                    cid: session_cid,
                    request_id: Some(request_id),
                    error: format!("Session {} not found", session_cid),
                },
            ),
            uuid: conn_id,
        });
    };
    if let Err(error) = decide_claim(owner, only_if_orphaned, conn_id, session_cid) {
        return Some(refusal(session_cid, request_id, conn_id, error));
    }

    // Steps 3-4: a session the agent is reconnecting has no SDK session yet, and is
    // held, not dead; any other must be live in the SDK (connection_management_claim_sdk.rs).
    let reconnecting =
        link_of(this, session_cid).is_some_and(|link| !policy::claim_requires_sdk_session(link));
    if !reconnecting {
        if let Some(refused) = refuse_unless_sdk_holds(this, conn_id, request_id, session_cid).await
        {
            return Some(refused);
        }
    }

    // Step 5: Session is valid in both internal service and SDK - decide and
    // re-point atomically. The owner is re-read here, under the write lock,
    // because the early check's answer may have been overtaken during the
    // Step 3 await (see `decide_claim`). `owner_of` cannot be reused for
    // this: it takes and releases its own locks, which is exactly the
    // check-then-act split being closed.
    let mut server_connection_map = this.server_connection_map.write();

    let owner_uuid = match server_connection_map.get(&session_cid) {
        Some(connection) => connection
            .associated_localhost_connection
            .load(Ordering::Relaxed),
        None => {
            // Removed during the await (logout or deregister landed first).
            return Some(refusal(
                session_cid,
                request_id,
                conn_id,
                format!("Session {} not found", session_cid),
            ));
        }
    };
    // Same lock order as DisconnectOrphan: connection map, then client map —
    // never the reverse, so no inversion deadlock.
    let owner_now = if this
        .tx_to_localhost_clients
        .read()
        .contains_key(&owner_uuid)
    {
        SessionOwner::Live(owner_uuid)
    } else {
        SessionOwner::Orphaned
    };
    if let Err(error) = decide_claim(owner_now, only_if_orphaned, conn_id, session_cid) {
        return Some(refusal(session_cid, request_id, conn_id, error));
    }

    // Find ALL sessions that share the same old TCP connection
    // This ensures all sessions from the same browser/client get updated together
    //
    // Except the nil marker, which is not a connection. `ReleaseSession` stamps
    // `Uuid::nil()` on every session it releases, whatever account it belonged
    // to, so "shares the old owner" is true of every released session on the
    // machine at once. Claiming one of them re-pointed all of them at the
    // claimer — including other accounts' — and `associated_localhost_connection`
    // is the field `send_response_for_session` routes by and the ownership gate
    // reads, so the claimer then received another account's P2P, file and media
    // notifications, and that account was refused its own session as "not
    // orphaned".
    //
    // The sweep is right for a real uuid: sessions that shared one browser
    // socket do belong together. Nil says only "nobody holds this", which is a
    // property, not an identity.
    let sessions_to_update: Vec<u64> = if owner_uuid.is_nil() {
        vec![session_cid]
    } else {
        server_connection_map
            .iter()
            .filter(|(_, conn)| {
                conn.associated_localhost_connection.load(Ordering::Relaxed) == owner_uuid
            })
            .map(|(cid, _)| *cid)
            .collect()
    };

    let updated_count = sessions_to_update.len();

    // Update all sessions that shared the old TCP connection to use the new one
    // NOTE: We do NOT clear peer connections - the SDK P2P connections are still
    // active even though the TCP connection to internal service was dropped.
    // The AsyncSink channels in PeerConnection are SDK-layer, not TCP-layer.
    for cid in &sessions_to_update {
        if let Some(conn) = server_connection_map.get_mut(cid) {
            conn.associated_localhost_connection
                .store(conn_id, Ordering::Relaxed);
            let peer_count = conn.peers.len();
            if peer_count > 0 {
                info!(target: "citadel", "ClaimSession: Session {} has {} existing peer connections (preserved)", cid, peer_count);
            }
        }
    }

    info!(target: "citadel", "ClaimSession: Updated {} sessions from old TCP connection {:?} to new {:?}", updated_count, owner_uuid, conn_id);

    // Add this connection to orphan mode to preserve it when the new connection drops
    this.orphan_sessions.write().insert(conn_id, true);
    drop(server_connection_map);

    // The reconnect's own notices went to the connection that is gone; the claimer
    // hears the link is down here, and "reconnected" or "failed" follows to it.
    if reconnecting {
        tell_the_claimer_it_is_reconnecting(this, session_cid, conn_id);
    }

    Some(HandledRequestResult {
        response: InternalServiceResponse::ConnectionManagementSuccess(
            ConnectionManagementSuccess {
                cid: session_cid,
                request_id: Some(request_id),
                message: format!(
                    "Successfully claimed session {} (updated {} related sessions)",
                    session_cid, updated_count
                ),
            },
        ),
        uuid: conn_id,
    })
}

#[cfg(test)]
#[path = "connection_management_claim_tests.rs"]
mod tests;

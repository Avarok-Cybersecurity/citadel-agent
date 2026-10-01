//! Claiming a session for this localhost connection.
//!
//! Split out of `connection_management.rs`: this arm is longer than the other
//! three put together, and the file was over the 250-line cap. The
//! authorization rule it enforces lives in `connection_management_auth.rs`,
//! shared with the other two session-mutating commands so the three cannot
//! drift apart.

use crate::kernel::reconnect::{policy, LinkState};
use crate::kernel::requests::connection_management::{live_owner, owner_of, refusal};
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

pub(super) async fn claim_session<T: IOInterface + Sync, R: Ratchet>(
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
    // Under the map's write lock, released before anything below awaits.
    let (sessions_to_update, updated_count) = {
        let server_connection_map = this.server_connection_map.write();

        let Some(subscribers) = server_connection_map
            .get(&session_cid)
            .map(|connection| connection.subscribers.clone())
        else {
            // Removed during the await (logout or deregister landed first).
            return Some(refusal(
                session_cid,
                request_id,
                conn_id,
                format!("Session {} not found", session_cid),
            ));
        };
        // Same lock order as DisconnectOrphan: connection map, then client map —
        // never the reverse, so no inversion deadlock.
        let owner_now = live_owner(this, subscribers.members());
        if let Err(error) = decide_claim(owner_now.clone(), only_if_orphaned, conn_id, session_cid)
        {
            return Some(refusal(session_cid, request_id, conn_id, error));
        }

        // An orphan is adopted together with every other orphan the SAME dropped
        // socket held: sessions that shared one browser socket belong together, so
        // a reload reclaims all of them at once. `last_holder` is that socket; it is
        // `None` after a release ("nobody's"), which is a property, not an identity
        // -- sweeping by it once adopted every released session on the machine,
        // other accounts' included.
        //
        // A member re-asserting a live session it is attached to changes nothing,
        // and in particular does not throw the session's other windows out.
        let sessions_to_update: Vec<(u64, crate::kernel::session_subscribers::SessionSubscribers)> =
            match (owner_now, subscribers.last_holder()) {
                (SessionOwner::Orphaned, Some(old_socket)) => server_connection_map
                    .iter()
                    .filter(|(_, conn)| {
                        conn.subscribers.last_holder() == Some(old_socket)
                            && conn.subscribers.primary().is_none()
                    })
                    .map(|(cid, conn)| (*cid, conn.subscribers.clone()))
                    .collect(),
                _ => vec![(session_cid, subscribers)],
            };

        let updated_count = sessions_to_update.len();

        // NOTE: We do NOT clear peer connections - the SDK P2P connections are still
        // active even though the TCP connection to internal service was dropped.
        // The AsyncSink channels in PeerConnection are SDK-layer, not TCP-layer.
        for (cid, subs) in &sessions_to_update {
            crate::kernel::membership::take_over_in(
                &this.tx_to_localhost_clients,
                *cid,
                subs,
                conn_id,
            );
            if let Some(conn) = server_connection_map.get(cid) {
                let peer_count = conn.peers.len();
                if peer_count > 0 {
                    info!(target: "citadel", "ClaimSession: Session {} has {} existing peer connections (preserved)", cid, peer_count);
                }
            }
        }

        info!(target: "citadel", "ClaimSession: connection {:?} now holds {} session(s)", conn_id, updated_count);

        // Add this connection to orphan mode to preserve it when the new connection drops
        this.orphan_sessions.write().insert(conn_id, true);

        (sessions_to_update, updated_count)
    };

    // The reconnect's own notices went to the connection that is gone; the claimer
    // hears the link is down here, and "reconnected" or "failed" follows to it.
    if reconnecting {
        tell_the_claimer_it_is_reconnecting(this, session_cid, conn_id);
    }
    for (cid, _) in &sessions_to_update {
        this.host_ilm_for(*cid, conn_id).await;
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

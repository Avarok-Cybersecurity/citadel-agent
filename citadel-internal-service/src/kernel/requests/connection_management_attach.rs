//! Joining a session another window holds, without taking it away.
//!
//! `ClaimSession` and a live `Connect` re-point a session to the caller, and the
//! window it was taken from stops receiving. `AttachSession` adds the caller to
//! the session's subscribers instead (kernel/session_subscribers.rs), so every
//! window keeps working.
//!
//! It always requires a proof, live session or orphan alike -- the UI only asks
//! when it has one, and an orphan can still be reclaimed proof-free through
//! `ClaimSession`, exactly as before. The proof is either:
//!
//! * the password, checked the way the live branch of `Connect` checks it:
//!   `credential_fingerprint::derive` against the fingerprint recorded when the
//!   session was opened. A success mints an attach token.
//! * a token an earlier password attach to this same session minted
//!   (kernel/attach_tokens.rs), so a browser that has joined once is not asked
//!   again while the session lives.
//!
//! A failed proof changes nothing.

use crate::kernel::credential_fingerprint;
use crate::kernel::requests::connection_management::{live_owner, refusal};
use crate::kernel::requests::connection_management_auth::SessionOwner;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::session_route::announce_roles;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    AttachProof, InternalServiceResponse, SessionAttached, SessionRole,
};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

/// Distinct, so a UI holding a stale token knows to drop it and ask for the
/// password, while a wrong password is reported as one.
pub(crate) const WRONG_PASSWORD: &str = "The password does not match this session";
pub(crate) const TOKEN_REFUSED: &str = "This browser's session token is not valid any more";

pub(super) async fn attach_session<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    conn_id: Uuid,
    request_id: Uuid,
    session_cid: u64,
    proof: AttachProof,
) -> Option<HandledRequestResult> {
    let not_found = || {
        refusal(
            session_cid,
            request_id,
            conn_id,
            format!("Session {session_cid} not found"),
        )
    };
    let Some(username) = this
        .server_connection_map
        .read()
        .get(&session_cid)
        .map(|conn| conn.username.clone())
    else {
        return Some(not_found());
    };

    // Derived with no lock held: it awaits the account manager.
    let presented = match &proof {
        AttachProof::Password(password) => Presented::Fingerprint(
            credential_fingerprint::derive(this.remote(), &username, password.clone()).await,
        ),
        AttachProof::Token(token) => Presented::Token(token.clone()),
    };

    let (subscribers, token, displaced) = {
        let mut map = this.server_connection_map.write();
        // Removed during the await (logout or deregister landed first).
        let Some(conn) = map.get_mut(&session_cid) else {
            return Some(not_found());
        };
        let proven = match &presented {
            Presented::Fingerprint(fingerprint) => credential_fingerprint::matches(
                conn.credential_fingerprint.as_ref(),
                fingerprint.as_ref(),
            ),
            Presented::Token(token) => conn.attach_tokens.admits(token),
        };
        if !proven {
            let error = match presented {
                Presented::Fingerprint(_) => WRONG_PASSWORD,
                Presented::Token(_) => TOKEN_REFUSED,
            };
            warn!(target: "citadel", "AttachSession REFUSED for session {session_cid} from connection {conn_id}: no valid proof");
            return Some(refusal(session_cid, request_id, conn_id, error.to_string()));
        }
        let token = match presented {
            Presented::Fingerprint(_) => match conn.attach_tokens.mint() {
                Some(token) => token,
                None => {
                    return Some(refusal(
                        session_cid,
                        request_id,
                        conn_id,
                        "The agent could not create a session token".to_string(),
                    ))
                }
            },
            Presented::Token(token) => token,
        };
        let subscribers = conn.subscribers.clone();
        // Nobody live holds it: the caller takes it, as a claim would. Otherwise
        // it joins the ones that do.
        let displaced = match live_owner(this, subscribers.members()) {
            SessionOwner::Orphaned => subscribers.take_over(conn_id),
            SessionOwner::Live(_) => {
                subscribers.attach(conn_id);
                Vec::new()
            }
        };
        (subscribers, token, displaced)
    };

    // Kept, as a claim keeps it, so a later drop of this connection preserves sessions.
    this.orphan_sessions.write().insert(conn_id, true);
    if subscribers.members().len() > 1 || !displaced.is_empty() {
        announce_roles(
            &this.tx_to_localhost_clients,
            session_cid,
            &subscribers,
            &displaced,
        );
    }
    this.host_ilm_for(session_cid, conn_id).await;
    let role = if subscribers.primary() == Some(conn_id) {
        SessionRole::Primary
    } else {
        SessionRole::Secondary
    };
    info!(target: "citadel", "AttachSession: connection {conn_id} joined session {session_cid} as {role:?}");

    Some(HandledRequestResult {
        response: InternalServiceResponse::SessionAttached(SessionAttached {
            cid: session_cid,
            role,
            token,
            request_id: Some(request_id),
        }),
        uuid: conn_id,
    })
}

enum Presented {
    Fingerprint(Option<Vec<u8>>),
    Token(Vec<u8>),
}

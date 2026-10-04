//! A user's sign-in taking over a session the agent is reconnecting.
//!
//! The server may hold the dead session for up to an hour after a one-sided drop, refusing
//! the reconnect all that time. A sign-in with `force_login` that authenticates replaces it
//! (Citadel-Protocol #319), so a user who signs in need not wait. The reconnect is stopped
//! first -- never two SDK connects for the account at once -- and the forced connect goes
//! into the SAME entry under the same CID, so the page's subscribers, pending invites and
//! groups carry over. What decides is sign_in.rs; this is the I/O.
//!
//! Handoff: the entry is marked `SigningIn` under the map lock (only from `Reconnecting`),
//! then the reconnect's attempt lock is taken, which waits out an attempt in flight. That
//! attempt, if it lands, is adopted (the entry is then `Up`, and the sign-in is answered
//! with it); every later check of the reconnect sees the mark and stops. If the forced
//! connect fails, the session goes back to a NEW reconnect run, and the stopped one, if it
//! was only asleep, sees the run number moved and stays stopped.

use super::link::put_link;
use super::report::logged;
use super::{sign_in, task, Credentials, LinkState, Reauth, LOG_TARGET};
use crate::kernel::{CitadelWorkspaceService, Connection};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    ConnectFailure, ConnectSuccess, InternalServiceResponse, SessionAlreadyActive,
};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{
    AuthenticationRequest, ProtocolRemoteExt, ProtocolRemoteTargetExt, Ratchet,
};
use uuid::Uuid;

/// Take `cid`'s reconnect over for the localhost connection `caller`, whose Connect
/// `request_id` proved the password (the caller checked the fingerprint). `credentials`
/// are the sign-in's, with the connect mode already forcing.
pub(crate) async fn take_over<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    caller: Uuid,
    request_id: Uuid,
    username: String,
    credentials: Credentials,
) -> InternalServiceResponse {
    let failure = |message: String| {
        InternalServiceResponse::ConnectFailure(ConnectFailure {
            cid,
            message,
            request_id: Some(request_id),
            reason_code: None,
        })
    };
    let success = InternalServiceResponse::ConnectSuccess(ConnectSuccess {
        cid,
        request_id: Some(request_id),
    });

    // The caller proved the password (its fingerprint matched), so the sign-in is a password
    // one; nothing else is ever taken over.
    let Reauth::Password(password) = credentials.reauth.clone() else {
        return failure(format!(
            "Session {cid} cannot be taken over without its user"
        ));
    };
    let gate = {
        let mut lock = this.server_connection_map.write();
        match lock.get_mut(&cid) {
            Some(conn) if sign_in::may_take_over(conn.link) => {
                conn.link = LinkState::SigningIn;
                conn.handoff.clone()
            }
            _ => {
                return failure(format!(
                    "Session {cid} is already being signed in to or ended"
                ))
            }
        }
    };
    let _no_attempt_in_flight = gate.hold_attempts().await;
    let clients = this.tx_to_localhost_clients.clone();
    info!(target: LOG_TARGET, "[Reconnect] {cid}: a sign-in is taking the reconnect over");

    // The attempt it waited on may have landed and been adopted.
    let link = {
        let lock = this.server_connection_map.read();
        lock.get(&cid).map(|conn| {
            if conn.link == LinkState::Up {
                crate::kernel::membership::take_over_in(&clients, cid, &conn.subscribers, caller);
            }
            conn.link
        })
    };
    match link {
        Some(LinkState::SigningIn) => {}
        Some(LinkState::Up) => {
            info!(target: LOG_TARGET, "[Reconnect] {cid}: the reconnect landed first; the sign-in gets it");
            return success;
        }
        // Signed out (a Disconnect removed it) while the sign-in waited.
        Some(LinkState::Reconnecting | LinkState::Ending) | None => {
            return failure(format!("Session {cid} was ended during the sign-in"));
        }
    }

    let settings = credentials.session_security_settings;
    let connect = this
        .remote()
        .connect(
            AuthenticationRequest::credentialed(username, password),
            credentials.connect_mode,
            credentials.udp_mode,
            credentials.keep_alive_timeout,
            settings,
            credentials.server_password.clone(),
        )
        .await;
    let connected = match connect {
        Ok(connected) if connected.cid == cid => connected,
        Ok(connected) => {
            logged(
                cid,
                "closing a link under another CID",
                connected.remote.disconnect().await,
            );
            let reason = format!("the server answered as {}", connected.cid);
            return hand_back(this, cid, caller, request_id, reason);
        }
        Err(err) => {
            let message = err.into_string();
            warn!(target: LOG_TARGET, "[Reconnect] {cid}: the sign-in's connect failed ({message}); reconnecting again");
            return hand_back(this, cid, caller, request_id, message);
        }
    };

    let admit = |conn: &mut Connection<R>| {
        if conn.link != LinkState::SigningIn {
            return false;
        }
        crate::kernel::membership::take_over_in(&clients, cid, &conn.subscribers, caller);
        conn.reconnect = credentials;
        true
    };
    if put_link(this, cid, connected, settings, request_id, admit).await {
        info!(target: LOG_TARGET, "[Reconnect] {cid} is back through a sign-in");
        success
    } else {
        failure(format!("Session {cid} was ended during the sign-in"))
    }
}

/// The sign-in's connect failed: the session is still wanted, so a new reconnect run takes
/// it on, and the caller is answered as a sign-in during a reconnect always was -- the
/// session exists and is theirs, still reconnecting -- rather than with a failure that
/// would send a reloaded page to the sign-in form while its session is on its way back.
/// (The UI already shows it reconnecting; nothing new is announced.)
fn hand_back<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    caller: Uuid,
    request_id: Uuid,
    reason: String,
) -> InternalServiceResponse {
    let clients = this.tx_to_localhost_clients.clone();
    let handed = {
        let mut lock = this.server_connection_map.write();
        match lock.get_mut(&cid) {
            Some(conn) if conn.link == LinkState::SigningIn => {
                conn.link = LinkState::Reconnecting;
                crate::kernel::membership::take_over_in(&clients, cid, &conn.subscribers, caller);
                Some((conn.handoff.next_generation(), conn.username.clone()))
            }
            _ => None,
        }
    };
    let Some((generation, username)) = handed else {
        return InternalServiceResponse::ConnectFailure(ConnectFailure {
            cid,
            message: format!("Session {cid} was ended during the sign-in"),
            request_id: Some(request_id),
            reason_code: None,
        });
    };
    task::resume(this, cid, generation);
    InternalServiceResponse::SessionAlreadyActive(SessionAlreadyActive {
        cid,
        username,
        message: format!(
            "Session is reconnecting; signing in could not reach the server yet ({reason})"
        ),
        request_id: Some(request_id),
    })
}

//! The SDK side of a reconnect: mark the session, retry per the policy, install the
//! new channel or give up. Every decision is policy.rs's.

use super::policy::{self, DropAction, GiveUp, Next, SERVER_RECONNECT};
use super::report::{fail, logged, notify};
use super::{Credentials, LinkState};
use crate::kernel::{
    c2s_reader, create_client_server_remote, group_channels, CitadelWorkspaceService, Connection,
};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceResponse, ServerConnectionLost, ServerReconnected,
};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{
    AuthenticationRequest, CitadelClientServerConnection, NetworkError, ProtocolRemoteExt,
    ProtocolRemoteTargetExt, Ratchet,
};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;

/// What an unrequested drop did to the session.
pub(crate) enum Began {
    /// Kept and marked; the caller notifies and spawns the reconnect.
    Reconnecting,
    AlreadyReconnecting,
    /// Being ended by its user: the caller removes it as it always did.
    Remove,
    /// Not in the map: nothing to keep.
    NotTracked,
}

/// Decide, and if it is a reconnect, mark the session before anything else can see it
/// `Up`. Its peers, groups and C2S transfers belonged to the dead link and go.
pub(crate) fn begin<R: Ratchet>(map: &Arc<RwLock<HashMap<u64, Connection<R>>>>, cid: u64) -> Began {
    let mut lock = map.write();
    let Some(conn) = lock.get_mut(&cid) else {
        return Began::NotTracked;
    };
    match policy::on_unrequested_drop(conn.link) {
        DropAction::AlreadyReconnecting => Began::AlreadyReconnecting,
        DropAction::Remove => Began::Remove,
        DropAction::Reconnect => {
            conn.link = LinkState::Reconnecting;
            conn.peers.clear();
            // Only the send halves live here, and dropping one sends nothing. The recv
            // half, whose drop sends `LeaveRoom`, is owned by its receiver task and ends
            // with the dead session, so the server still lists this member and prompts
            // the new session to rejoin; `responses/group_channel_created.rs` adopts the
            // channel that rejoin opens, into this same (never removed) entry.
            conn.groups = group_channels::GroupChannels::new();
            conn.c2s_file_transfer_handlers.clear();
            Began::Reconnecting
        }
    }
}

/// Tell the UI and start retrying. Called once, after `begin` answered `Reconnecting`.
pub(crate) fn spawn<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
) -> Result<(), NetworkError> {
    notify(
        this,
        cid,
        InternalServiceResponse::ServerConnectionLost(ServerConnectionLost {
            cid,
            reconnecting: true,
            request_id: None,
        }),
    )?;
    let this = this.clone();
    drop(tokio::spawn(async move { run(&this, cid).await }));
    Ok(())
}

async fn run<T: IOInterface + Sync, R: Ratchet>(this: &CitadelWorkspaceService<T, R>, cid: u64) {
    let started = Instant::now();
    let mut attempt: u32 = 0;
    let mut delay = SERVER_RECONNECT.delay_before(attempt);
    loop {
        tokio::time::sleep(delay).await;
        let Some((username, credentials)) = still_reconnecting(&this.server_connection_map, cid)
        else {
            info!(target: "citadel", "[Reconnect] {cid} was ended while reconnecting; stopping");
            return;
        };
        let connect_request_id = credentials.connect_request_id;
        let settings = credentials.session_security_settings;
        let failure = match attempt_once(this, username, credentials).await {
            Ok(connected) if connected.cid == cid => {
                return install(this, cid, connected, settings, connect_request_id).await;
            }
            Ok(connected) => {
                // Should be impossible (a CID is permanent per account); never adopt it.
                logged(
                    cid,
                    "closing a link under another CID",
                    connected.remote.disconnect().await,
                );
                return fail(
                    this,
                    cid,
                    format!("the server answered as {}", connected.cid),
                );
            }
            Err(err) => err,
        };
        let code = failure.code();
        let message = failure.into_string();
        let kind = policy::classify(code, &message);
        warn!(target: "citadel", "[Reconnect] attempt {attempt} for {cid} failed ({kind:?}, {code:?}): {message}");
        match SERVER_RECONNECT.after_failure(attempt, started.elapsed(), kind) {
            Next::RetryAfter(next) => {
                attempt = attempt.saturating_add(1);
                delay = next;
            }
            Next::GiveUp(GiveUp::Refused) => return fail(this, cid, message),
            Next::GiveUp(GiveUp::OutOfTime) => {
                let budget = SERVER_RECONNECT.give_up_after;
                return fail(
                    this,
                    cid,
                    format!("no answer from the server in {budget:?}: {message}"),
                );
            }
        }
    }
}

fn still_reconnecting<R: Ratchet>(
    map: &Arc<RwLock<HashMap<u64, Connection<R>>>>,
    cid: u64,
) -> Option<(String, Credentials)> {
    let lock = map.read();
    let conn = lock.get(&cid)?;
    (conn.link == LinkState::Reconnecting).then(|| (conn.username.clone(), conn.reconnect.clone()))
}

async fn attempt_once<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    username: String,
    credentials: Credentials,
) -> Result<CitadelClientServerConnection<R>, NetworkError> {
    let connect = this.remote().connect(
        AuthenticationRequest::credentialed(username, credentials.password),
        credentials.connect_mode,
        credentials.udp_mode,
        credentials.keep_alive_timeout,
        credentials.session_security_settings,
        credentials.server_password,
    );
    match tokio::time::timeout(SERVER_RECONNECT.attempt_timeout, connect).await {
        Ok(result) => result,
        Err(_) => Err(NetworkError::timeout(
            SERVER_RECONNECT.attempt_timeout.as_secs(),
        )),
    }
}

async fn install<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    connected: CitadelClientServerConnection<R>,
    settings: citadel_sdk::prelude::SessionSecuritySettings,
    connect_request_id: Uuid,
) {
    let (sink, stream) = connected.split();
    let remote = create_client_server_remote(stream.vconn_type, this.remote().clone(), settings);
    let installed = {
        let mut lock = this.server_connection_map.write();
        match lock.get_mut(&cid) {
            Some(conn) if conn.link == LinkState::Reconnecting => {
                conn.sink_to_server = Arc::new(tokio::sync::Mutex::new(sink));
                conn.client_server_remote = remote.clone();
                conn.link = LinkState::Up;
                Some(conn.associated_localhost_connection.load(Ordering::Relaxed))
            }
            _ => None,
        }
    };
    let Some(tcp_uuid) = installed else {
        // Ended while this attempt was in flight: the session it opened has no owner.
        info!(target: "citadel", "[Reconnect] {cid} was ended mid-attempt; closing the new link");
        logged(cid, "closing an unowned link", remote.disconnect().await);
        return;
    };
    c2s_reader::spawn(
        this.server_connection_map.clone(),
        this.tx_to_localhost_clients.clone(),
        cid,
        stream,
        connect_request_id,
        tcp_uuid,
    );
    info!(target: "citadel", "[Reconnect] {cid} is back");
    let sent = notify(
        this,
        cid,
        InternalServiceResponse::ServerReconnected(ServerReconnected {
            cid,
            request_id: None,
        }),
    );
    logged(cid, "sending ServerReconnected", sent);
}

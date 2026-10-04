//! The SDK side of a reconnect: mark the session, retry per the policy, install the
//! new channel or give up. Every decision is policy.rs's.

use super::attempt;
use super::link::put_link;
use super::lost_peers;
use super::policy::{self, DropAction, FailureKind, GiveUp, Next};
use super::report::{fail, logged, notify};
use super::sign_in;
use super::{Credentials, Handoff, LinkState, Reauth, LOG_TARGET};
use crate::kernel::{group_channels, CitadelWorkspaceService, Connection};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{InternalServiceResponse, ServerConnectionLost};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{
    CitadelClientServerConnection, NetworkError, ProtocolRemoteTargetExt, Ratchet,
};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;

/// What an unrequested drop did to the session.
pub(crate) enum Began {
    /// Kept and marked; the caller notifies (of `lost_peers` too) and spawns the reconnect.
    Reconnecting {
        lost_peers: Vec<u64>,
    },
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
            conn.handoff.next_generation();
            let lost_peers = lost_peers::take(&mut conn.peers);
            // Only the send halves live here, and dropping one sends nothing. The recv
            // half, whose drop sends `LeaveRoom`, is owned by its receiver task and ends
            // with the dead session, so the server still lists this session's groups:
            // it prompts the new session to rejoin those it joined and to re-found those
            // it owns, and `responses/group_channel_created.rs` adopts the channel each
            // one opens into this same (never removed) entry.
            conn.groups = group_channels::GroupChannels::new();
            conn.c2s_file_transfer_handlers.clear();
            Began::Reconnecting { lost_peers }
        }
    }
}

/// Tell the UI and start retrying. Called once, after `begin` answered `Reconnecting`.
pub(crate) fn spawn<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    lost_peers: &[u64],
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
    for notice in lost_peers::notices(cid, lost_peers) {
        notify(this, cid, notice)?;
    }
    let generation = {
        let lock = this.server_connection_map.read();
        lock.get(&cid).map(|conn| conn.handoff.generation())
    };
    if let Some(generation) = generation {
        resume(this, cid, generation);
    }
    Ok(())
}

/// Start reconnect run `generation` for `cid`, telling the UI nothing: it already knows
/// the session is reconnecting (a takeover that failed hands it back through here).
pub(crate) fn resume<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    generation: u64,
) {
    let this = this.clone();
    drop(tokio::spawn(
        async move { run(&this, cid, generation).await },
    ));
}

/// The session's handoff, whose attempt lock a takeover takes to wait out an attempt in
/// flight (see `Handoff`).
fn attempt_gate<R: Ratchet>(
    map: &Arc<RwLock<HashMap<u64, Connection<R>>>>,
    cid: u64,
) -> Option<Handoff> {
    map.read().get(&cid).map(|conn| conn.handoff.clone())
}

async fn run<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    generation: u64,
) {
    let started = Instant::now();
    let mut attempt: u32 = 0;
    let mut delay = this.reconnect_policy.delay_before(attempt);
    loop {
        tokio::time::sleep(delay).await;
        let Some(gate) = attempt_gate(&this.server_connection_map, cid) else {
            info!(target: LOG_TARGET, "[Reconnect] {cid} was ended while reconnecting; stopping");
            return;
        };
        // Held until this attempt's outcome is installed or discarded.
        let _attempt = gate.hold_attempts().await;
        let Some((username, credentials)) =
            still_reconnecting(&this.server_connection_map, cid, generation)
        else {
            info!(target: LOG_TARGET, "[Reconnect] {cid} is no longer this run's to reconnect (ended, signed in to, or handed to a newer run); stopping");
            return;
        };
        let password = match credentials.reauth.clone() {
            Reauth::Password(password) => password,
            Reauth::NeedsUser(reason) => return fail(this, cid, generation, reason.to_string()),
        };
        let policy = this
            .reconnect_policy
            .for_keep_alive(credentials.keep_alive_timeout);
        let connect_request_id = credentials.connect_request_id;
        let settings = credentials.session_security_settings;
        let failure = match attempt::once(this, &policy, username, password, credentials).await {
            Ok(connected) if connected.cid == cid => {
                return install(
                    this,
                    cid,
                    generation,
                    connected,
                    settings,
                    connect_request_id,
                )
                .await;
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
                    generation,
                    format!("the server answered as {}", connected.cid),
                );
            }
            Err(err) => err,
        };
        let code = failure.code();
        let message = failure.into_string();
        let kind = policy::classify(code, &message);
        warn!(target: LOG_TARGET, "[Reconnect] attempt {attempt} for {cid} failed ({kind:?}, {code:?}): {message}");
        match policy.after_failure(attempt, started.elapsed(), kind) {
            Next::RetryAfter(next) => {
                attempt = attempt.saturating_add(1);
                delay = next;
            }
            Next::GiveUp(GiveUp::Refused) => return fail(this, cid, generation, message),
            Next::GiveUp(GiveUp::OutOfTime) => {
                let budget = policy.limit(kind);
                let reason = match kind {
                    FailureKind::ServerHoldsSession => {
                        format!("the server still held the previous session after {budget:?}: {message}")
                    }
                    FailureKind::Refused | FailureKind::Transient => {
                        format!("no answer from the server in {budget:?}: {message}")
                    }
                };
                return fail(this, cid, generation, reason);
            }
        }
    }
}

fn still_reconnecting<R: Ratchet>(
    map: &Arc<RwLock<HashMap<u64, Connection<R>>>>,
    cid: u64,
    generation: u64,
) -> Option<(String, Credentials)> {
    let lock = map.read();
    let conn = lock.get(&cid)?;
    sign_in::attempt_wanted(conn.link, conn.handoff.generation(), generation)
        .then(|| (conn.username.clone(), conn.reconnect.clone()))
}

async fn install<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    generation: u64,
    connected: CitadelClientServerConnection<R>,
    settings: citadel_sdk::prelude::SessionSecuritySettings,
    connect_request_id: Uuid,
) {
    // A sign-in that began taking over while this attempt was in flight is still waiting
    // for it: the session this attempt opened is the one it wants, so it is adopted, and
    // the sign-in finds the entry `Up` (reconnect/takeover.rs).
    let admit = |conn: &mut Connection<R>| {
        sign_in::attempt_installs(conn.link, conn.handoff.generation(), generation)
    };
    if put_link(this, cid, connected, settings, connect_request_id, admit).await {
        info!(target: LOG_TARGET, "[Reconnect] {cid} is back");
    } else {
        info!(target: LOG_TARGET, "[Reconnect] {cid} was ended mid-attempt; closed the new link");
    }
}

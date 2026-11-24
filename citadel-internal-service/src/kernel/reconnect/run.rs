//! One reconnect run: retry per the policy, install the new channel or give up. Every
//! decision is policy.rs's.

use super::attempt;
use super::link::put_link;
use super::policy::{self, FailureKind, GiveUp, Next};
use super::report::{fail, logged};
use super::sign_in;
use super::{Credentials, Handoff, LOG_TARGET};
use crate::kernel::{CitadelWorkspaceService, Connection};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{CitadelClientServerConnection, ProtocolRemoteTargetExt, Ratchet};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;

/// The session's handoff, whose attempt lock a takeover takes to wait out an attempt in
/// flight (see `Handoff`).
fn attempt_gate<R: Ratchet>(
    map: &Arc<RwLock<HashMap<u64, Connection<R>>>>,
    cid: u64,
) -> Option<Handoff> {
    map.read().get(&cid).map(|conn| conn.handoff.clone())
}

pub(super) async fn run<T: IOInterface + Sync, R: Ratchet>(
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
        let auth = match attempt::sign_in_again(username, &credentials.reauth) {
            Ok(auth) => auth,
            Err(reason) => return fail(this, cid, generation, reason.to_string(), None),
        };
        let policy = this
            .reconnect_policy
            .for_keep_alive(credentials.keep_alive_timeout);
        let connect_request_id = credentials.connect_request_id;
        let settings = credentials.session_security_settings;
        let failure = match attempt::once(this, &policy, auth, credentials).await {
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
                    None,
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
            Next::GiveUp(GiveUp::Refused) => {
                let reason_code = policy::refusal(code, &message).and_then(policy::refusal_reason);
                return fail(this, cid, generation, message, reason_code);
            }
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
                let reason_code = policy::out_of_time_reason(kind);
                return fail(this, cid, generation, reason, reason_code);
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

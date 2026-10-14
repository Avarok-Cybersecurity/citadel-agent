//! The uplink's IO shell in the kernel: the C2S sinks it sends on, the flush timer, and the
//! session and peer events that tell the collector who exists and what names to redact.

use super::{SessionLink, LOG_TARGET};
use crate::kernel::{CitadelWorkspaceService, Connection};
use crate::telemetry::{KnownValue, RedactionKind, TelemetryError};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::{Ratchet, SecurityLevel, SessionScope};
use futures::future::BoxFuture;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::MissedTickBehavior;

/// Telemetry is diagnostics, not content: the level every other agent-originated C2S frame
/// uses (kernel/conversations_io.rs).
const UPLOAD_SECURITY_LEVEL: SecurityLevel = SecurityLevel::Standard;
/// Bounds one send, as a window's C2S message is bounded (requests/message.rs): a wedged
/// link fails the frame rather than stalling every session's flush behind it.
const SEND_TIMEOUT: Duration = crate::kernel::requests::message::PEER_SEND_TIMEOUT;

/// Every session's C2S sink, looked up per frame so a reconnect's new sink is used.
pub(crate) struct ServerLinks<R: Ratchet> {
    map: Arc<RwLock<HashMap<u64, Connection<R>>>>,
}

impl<R: Ratchet> ServerLinks<R> {
    pub(crate) fn new(map: Arc<RwLock<HashMap<u64, Connection<R>>>>) -> Self {
        Self { map }
    }
}

impl<R: Ratchet> SessionLink for ServerLinks<R> {
    fn send_to_server(&self, cid: u64, frame: Vec<u8>) -> BoxFuture<'static, Result<(), String>> {
        let sink = self
            .map
            .read()
            .get(&cid)
            .map(|conn| conn.sink_to_server.clone());
        Box::pin(async move {
            let sink = sink.ok_or_else(|| format!("session {cid} has no C2S link"))?;
            let sent = tokio::time::timeout(SEND_TIMEOUT, async {
                let mut sink = sink.lock().await;
                sink.set_security_level(UPLOAD_SECURITY_LEVEL);
                sink.send(frame).await
            })
            .await;
            match sent {
                Ok(Ok(())) => Ok(()),
                Ok(Err(err)) => Err(format!("{err:?}")),
                Err(_) => Err(format!("send timed out after {SEND_TIMEOUT:?}")),
            }
        })
    }
}

/// Flushes every `flush_interval` for the life of the service; never resolves without an
/// uplink, so the kernel's `select!` keeps running.
pub(crate) async fn run<T, R: Ratchet>(this: CitadelWorkspaceService<T, R>) {
    let Some(uplink) = this.telemetry.clone() else {
        return std::future::pending().await;
    };
    let links = ServerLinks::new(this.server_connection_map.clone());
    let mut timer = tokio::time::interval(uplink.flush_interval());
    timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
    timer.tick().await;
    loop {
        timer.tick().await;
        uplink.flush(&links).await;
    }
}

fn known(kind: RedactionKind, value: &str) -> Option<KnownValue> {
    (!value.trim().is_empty()).then(|| KnownValue {
        kind,
        value: value.to_owned(),
    })
}

impl<T: IOInterface, R: Ratchet> CitadelWorkspaceService<T, R> {
    /// A session is up, new or reconnected: record what identifies it, then ask its
    /// server whether it uploads. Re-registering keeps an existing opt-in and ring. A
    /// recovery session may not message its server, so it is not tracked.
    pub(crate) async fn telemetry_session_started(&self, cid: u64) {
        let Some(uplink) = self.telemetry.clone() else {
            return;
        };
        let session = self
            .server_connection_map
            .read()
            .get(&cid)
            .filter(|conn| conn.sign_in.scope == SessionScope::Full)
            .map(|conn| {
                (
                    conn.username.clone(),
                    conn.server_address.clone(),
                    conn.server_host.clone(),
                )
            });
        let Some((username, server_address, server_host)) = session else {
            return;
        };
        let full_name = match self
            .remote()
            .account_manager()
            .get_persistence_handler()
            .get_clients_metadata(None)
            .await
        {
            Ok(accounts) => accounts
                .into_iter()
                .find(|account| account.cid == cid)
                .map(|account| account.full_name),
            Err(err) => {
                warn!(target: LOG_TARGET, "Session {cid}'s full name is unknown to telemetry redaction: {err:?}");
                None
            }
        };
        let identity = [
            known(RedactionKind::Username, &username),
            full_name.and_then(|name| known(RedactionKind::FullName, &name)),
            known(RedactionKind::Host, &server_address),
            server_host.and_then(|host| known(RedactionKind::Host, &host)),
        ];
        uplink
            .collector()
            .register_session(cid, identity.into_iter().flatten().collect());
        let links = ServerLinks::new(self.server_connection_map.clone());
        uplink.ask_consent(&links, cid).await;
    }
}

impl<T, R: Ratchet> CitadelWorkspaceService<T, R> {
    /// A peer of `session_cid` was named: its username and full name are redacted from
    /// every line from now on.
    pub(crate) fn telemetry_learn_peer(
        &self,
        session_cid: u64,
        peer_cid: u64,
        username: Option<&str>,
        full_name: Option<&str>,
    ) {
        let Some(uplink) = self.telemetry.as_ref() else {
            return;
        };
        let identity: Vec<KnownValue> = [
            username.and_then(|name| known(RedactionKind::Username, name)),
            full_name.and_then(|name| known(RedactionKind::FullName, name)),
        ]
        .into_iter()
        .flatten()
        .collect();
        if identity.is_empty() {
            return;
        }
        match uplink
            .collector()
            .learn_peer(session_cid, peer_cid, identity)
        {
            // A session telemetry does not track, such as a recovery session.
            Ok(()) | Err(TelemetryError::UnknownSession(_)) => {}
            Err(err) => warn!(target: LOG_TARGET, "Peer {peer_cid} not learned: {err}"),
        }
    }

    /// The session ended: its ring and identities go with it.
    pub(crate) fn telemetry_forget_session(&self, cid: u64) {
        if let Some(uplink) = self.telemetry.as_ref() {
            uplink.collector().forget_session(cid);
        }
    }
}

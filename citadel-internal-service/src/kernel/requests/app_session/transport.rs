//! The `SessionTransport` the agent ships: the app session's account is a V2 identity account
//! signed in through the ordinary identity requests, so it is an ordinary session of the
//! agent's, reconnected under its CID by the agent's own reconnect when its server drops it.
//!
//! The account's only subscriber is a route of its own (a localhost-client slot no window
//! holds): what the session machinery tells it (kernel/apps/session/events.rs) becomes the
//! member's `AppSessionMessage` and `AppSessionState`, sent to the member session's windows.

use crate::kernel::apps::session::core::{AppAccount, SessionTransport};
use crate::kernel::apps::session::events;
use crate::kernel::apps::session::rules::{app_handle, Refusal};
use crate::kernel::identity::pin;
use crate::kernel::requests::identity::register::Registration;
use crate::kernel::requests::{disconnect, identity, message};
use crate::kernel::session_route::SessionRoute;
use crate::kernel::CitadelWorkspaceService;
use citadel_identity::{canonical_domain, make_username};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    FailureReason, InternalServiceRequest, InternalServiceResponse, SecurityLevel,
    SessionSecuritySettings,
};
use citadel_sdk::logging::info;
use citadel_sdk::prelude::Ratchet;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use uuid::Uuid;

pub(super) struct ServiceTransport<'a, T, R: Ratchet> {
    pub(super) this: &'a CitadelWorkspaceService<T, R>,
}

fn unavailable(what: &str, detail: impl std::fmt::Display) -> Refusal {
    Refusal::Unavailable(format!("{what}: {detail}"))
}

impl<T: IOInterface + Sync, R: Ratchet> ServiceTransport<'_, T, R> {
    /// The member's identity, which must be the unlocked one and must have registered the
    /// member's own account: a legacy member has none, and the guest falls back.
    async fn identity_handle(&self, account: &AppAccount) -> Result<String, Refusal> {
        if self.this.app_sessions.is_none() {
            return Err(Refusal::NotConfigured);
        }
        let host = identity::host(self.this).map_err(|_| Refusal::NotConfigured)?;
        let unlocked = host.unlocked.current().ok_or(Refusal::IdentityLocked)?;
        let member = pin::load(self.this.remote(), account.owner.member_cid)
            .await
            .map_err(|err| unavailable("reading the member's pin", err))?;
        match member {
            Some(pin) if pin.id_fp == unlocked.id_fp() => {}
            _ => return Err(Refusal::NoIdentity),
        }
        app_handle(&unlocked.id_fp(), &account.owner.install_id).ok_or(Refusal::Invalid)
    }

    /// What the member's own session was opened with: the account is held beside it, alike.
    fn member_settings(&self, member_cid: u64) -> Result<SessionSecuritySettings, Refusal> {
        let map = self.this.server_connection_map.read();
        let conn = map.get(&member_cid).ok_or(Refusal::Session)?;
        Ok(conn.reconnect.session_security_settings)
    }

    /// Register the account the first time, else sign in by its CID: never a second account.
    /// Registered and signed in under the canonical domain; dialled where the configuration
    /// says, and signed in to again only while it still says so.
    async fn sign_in(
        &self,
        route: Uuid,
        account: &AppAccount,
        handle: String,
    ) -> Result<InternalServiceResponse, Refusal> {
        let settings = self.member_settings(account.owner.member_cid)?;
        let config = self
            .this
            .app_sessions
            .as_ref()
            .ok_or(Refusal::NotConfigured)?;
        let domain = canonical_domain(&account.server).map_err(|_| Refusal::Invalid)?;
        let username = make_username(&handle, &domain).map_err(|_| Refusal::Invalid)?;
        let remote = self.this.remote();
        let held = crate::kernel::account_server::held(remote, &username)
            .await
            .map_err(|err| unavailable("reading the agent's accounts", err))?;
        let request_id = Uuid::new_v4();
        let answered = if let Some(held) = held {
            let cid = remote
                .account_manager()
                .get_persistence_handler()
                .get_cid_by_username(&username);
            let record = pin::load(remote, cid)
                .await
                .map_err(|err| unavailable("reading the account's pin", err))?;
            if let Some(record) = record {
                config
                    .sign_in_again(domain.as_str(), &held.as_requested(), &record)
                    .map_err(|why| unavailable(&username, why))?;
            }
            identity::connect::connect_identity(self.this, route, request_id, cid, settings, None)
                .await
        } else {
            let registration = Registration {
                request_id,
                server_addr: domain.to_string(),
                full_name: handle.clone(),
                handle,
                discovery: config.discovery(domain.as_str()),
                session_security_settings: settings,
                server_password: None,
                admission_token: None,
            };
            identity::register::register(self.this, route, registration).await
        };
        answered
            .map(|answered| answered.response)
            .ok_or_else(|| unavailable("signing in", "no answer"))
    }
}

/// The account's CID from what its sign-in answered.
fn signed_in(answer: InternalServiceResponse) -> Result<u64, Refusal> {
    match answer {
        InternalServiceResponse::ConnectSuccess(ok) => Ok(ok.cid),
        InternalServiceResponse::SessionAlreadyActive(live) => Ok(live.cid),
        InternalServiceResponse::ConnectFailure(f)
            if f.reason_code == Some(FailureReason::IdentityLocked) =>
        {
            Err(Refusal::IdentityLocked)
        }
        InternalServiceResponse::ConnectFailure(f) => Err(unavailable("connect", f.message)),
        InternalServiceResponse::RegisterFailure(f) => Err(unavailable("register", f.message)),
        other => Err(unavailable("signing in", format!("{other:?}"))),
    }
}

#[async_trait::async_trait]
impl<T: IOInterface + Sync, R: Ratchet> SessionTransport for ServiceTransport<'_, T, R> {
    async fn open(&self, account: &AppAccount) -> Result<u64, Refusal> {
        let handle = self.identity_handle(account).await?;
        let route = Uuid::new_v4();
        let (tx, rx) = unbounded_channel();
        self.this.tx_to_localhost_clients.write().insert(route, tx);
        let answer = self.sign_in(route, account, handle).await;
        match answer.and_then(signed_in) {
            Ok(server_cid) => {
                // `connected` is recorded by the caller once this returns; the route reads
                // the CID at each event, so nothing before that is mistaken for the account's.
                spawn_route(self.this.clone(), route, account.session, rx);
                Ok(server_cid)
            }
            Err(refusal) => {
                self.this.tx_to_localhost_clients.write().remove(&route);
                Err(refusal)
            }
        }
    }

    async fn send(&self, server_cid: u64, body: Vec<u8>) -> Result<(), Refusal> {
        let request = InternalServiceRequest::Message {
            request_id: Uuid::new_v4(),
            message: body,
            cid: server_cid,
            peer_cid: None,
            security_level: SecurityLevel::Standard,
        };
        let answered = message::handle(self.this, Uuid::nil(), request).await;
        match answered.map(|a| a.response) {
            Some(InternalServiceResponse::MessageSendSuccess(_)) => Ok(()),
            Some(InternalServiceResponse::MessageSendFailure(f)) => {
                Err(unavailable("send", f.message))
            }
            other => Err(unavailable("send", format!("{other:?}"))),
        }
    }

    async fn close(&self, server_cid: u64) -> Result<(), Refusal> {
        let request = InternalServiceRequest::Disconnect {
            request_id: Uuid::new_v4(),
            cid: server_cid,
        };
        // Its route ends on the `DisconnectNotification`; one that heard none is let go here.
        let routes = crate::kernel::membership::subscribers_of(self.this, server_cid);
        let _answer = disconnect::handle(self.this, Uuid::nil(), request).await;
        if let Some(routes) = routes {
            let mut clients = self.this.tx_to_localhost_clients.write();
            for route in routes.members() {
                clients.remove(&route);
            }
        }
        Ok(())
    }
}

/// Turns what the account's session machinery reports into the member's app session events,
/// until the session is gone; then gives up its slot.
fn spawn_route<T: IOInterface + Sync, R: Ratchet>(
    this: CitadelWorkspaceService<T, R>,
    route: Uuid,
    session: u32,
    mut rx: UnboundedReceiver<InternalServiceResponse>,
) {
    drop(tokio::spawn(async move {
        let sessions = &this.apps.sessions;
        while let Some(response) = rx.recv().await {
            let Some(server_cid) = sessions.server_cid_of(session) else {
                break;
            };
            let Some(event) = events::translate(server_cid, response) else {
                continue;
            };
            let Some((member_cid, out)) = sessions.on_event(session, event) else {
                break;
            };
            let delivered =
                crate::kernel::membership::subscribers_of(&this, member_cid).map(|subs| {
                    SessionRoute::new(subs, this.tx_to_localhost_clients.clone()).send(out)
                });
            if delivered.is_none_or(|reached| reached.is_empty()) {
                info!(target: "citadel", "[APP-SESSION] {member_cid}: no window to hear session {session}");
            }
        }
        this.tx_to_localhost_clients.write().remove(&route);
    }));
}

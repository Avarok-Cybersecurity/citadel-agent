//! One app session request, decided against the registry and carried out through the
//! `SessionTransport` port. No SDK here: the port is the agent's session machinery.

use super::rules::Refusal;
use super::{AppSessions, Owner, Reserved};
use crate::kernel::apps::net::authority::KernelGrant;
use crate::kernel::apps::net::grants::GrantFault;
use citadel_internal_service_types::SessionStatus;

/// The account the transport signs in for a new session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppAccount {
    pub owner: Owner,
    pub session: u32,
    /// Canonical, and granted.
    pub server: String,
}

/// The agent's C2S machinery, as an app session uses it.
#[async_trait::async_trait]
pub(crate) trait SessionTransport: Send + Sync {
    /// Sign in the agent-held account for `account`, registering it the first time; its
    /// link's events go to `account.session`. The account's CID on the server.
    async fn open(&self, account: &AppAccount) -> Result<u64, Refusal>;
    async fn send(&self, server_cid: u64, body: Vec<u8>) -> Result<(), Refusal>;
    async fn close(&self, server_cid: u64) -> Result<(), Refusal>;
}

/// What an open answers: the session, its server CID (0 while connecting) and its state.
pub(crate) type Opened = (u32, u64, SessionStatus);

/// `grant` is the server's word on the install, as resolved for this member.
pub(crate) async fn open(
    sessions: &AppSessions,
    transport: &dyn SessionTransport,
    grant: Result<Option<KernelGrant>, GrantFault>,
    app_id: &str,
    owner: Owner,
    server: &str,
) -> Result<Opened, Refusal> {
    let grant = grant.map_err(|_| Refusal::Authority)?;
    match sessions.reserve(grant.as_ref(), app_id, &owner, server)? {
        Reserved::Existing {
            session,
            server_cid,
            status,
        } => Ok((session, server_cid, status)),
        Reserved::New { session, server } => {
            let account = AppAccount {
                owner,
                session,
                server,
            };
            match transport.open(&account).await {
                Ok(server_cid) => {
                    sessions.connected(session, server_cid);
                    Ok((session, server_cid, SessionStatus::Connected))
                }
                Err(refusal) => {
                    sessions.remove(session);
                    Err(refusal)
                }
            }
        }
    }
}

pub(crate) async fn send(
    sessions: &AppSessions,
    transport: &dyn SessionTransport,
    member_cid: u64,
    install_id: &str,
    session: u32,
    body: Vec<u8>,
) -> Result<(), Refusal> {
    let server_cid = sessions.decide_send(member_cid, install_id, session, body.len())?;
    transport.send(server_cid, body).await
}

/// Taken out of the registry first, so the link's last words about it reach nobody.
pub(crate) async fn close(
    sessions: &AppSessions,
    transport: &dyn SessionTransport,
    member_cid: u64,
    install_id: &str,
    session: u32,
) -> Result<(), Refusal> {
    if !sessions.is_owned(member_cid, install_id, session) {
        return Err(Refusal::Invalid);
    }
    match sessions.remove(session) {
        Some(server_cid) if server_cid != 0 => transport.close(server_cid).await,
        _ => Ok(()),
    }
}

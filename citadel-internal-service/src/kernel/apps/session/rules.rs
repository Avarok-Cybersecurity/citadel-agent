//! What the agent decides about an app session before anything is dialled. Pure.
//!
//! The workspace server's grant is the authority (kernel/apps/net/authority.rs): `servers` is
//! the manifest's list it approved `citadel.session` to, `session` its `limits.session`. The
//! host enforces the same rules first (`session-rules.ts`); these are the agent's own, so a
//! page cannot reach a server the install was not approved for by skipping the host.

use crate::kernel::apps::net::authority::{KernelGrant, SessionLimits};
use citadel_identity::canonical_domain;
use citadel_internal_service_types::{
    SessionStatus, APP_SESSION_MAX_BODY_BYTES, APP_SESSION_MAX_SERVERS,
};

/// Why a session request is refused; its wire name is `AppSessionFailure.reason`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    Ungranted,
    Limit,
    Invalid,
    NoIdentity,
    IdentityLocked,
    NotConfigured,
    Session,
    Authority,
    /// Allowed, and the external server could not be reached; the detail is logged only.
    Unavailable(String),
}

impl Refusal {
    pub(crate) fn wire(&self) -> &'static str {
        use citadel_internal_service_types::app_session_reason as r;
        match self {
            Self::Ungranted => r::UNGRANTED,
            Self::Limit => r::LIMIT,
            Self::Invalid => r::INVALID,
            Self::NoIdentity => r::NO_IDENTITY,
            Self::IdentityLocked => r::IDENTITY_LOCKED,
            Self::NotConfigured => r::NOT_CONFIGURED,
            Self::Session => r::SESSION,
            Self::Authority => r::AUTHORITY,
            Self::Unavailable(_) => r::UNAVAILABLE,
        }
    }
}

/// One session this install already holds for this member and acting user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Held {
    pub server: String,
    pub session: u32,
    pub status: SessionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OpenDecision {
    /// The live session to this server: a second open returns it.
    Reuse(u32),
    /// A new session to `server` (canonical), under the install's `limits`. `replaces` is a
    /// closed session to the same server, whose slot it takes.
    Open {
        server: String,
        limits: SessionLimits,
        replaces: Option<u32>,
    },
}

/// `grant` is the server's word on the install (`None`: it allows nothing); `held` are the
/// install's sessions for this member and acting user.
pub(crate) fn decide_open(
    grant: Option<&KernelGrant>,
    app_id: &str,
    server: &str,
    held: &[Held],
) -> Result<OpenDecision, Refusal> {
    let grant = grant.ok_or(Refusal::Ungranted)?;
    if grant.app_id != app_id {
        return Err(Refusal::Ungranted);
    }
    let limits = grant.session.ok_or(Refusal::Ungranted)?;
    let server = canonical_domain(server.trim())
        .map_err(|_| Refusal::Invalid)?
        .to_string();
    let granted = grant
        .servers
        .iter()
        .any(|allowed| canonical_domain(allowed).is_ok_and(|allowed| allowed.as_str() == server));
    if !granted {
        return Err(Refusal::Ungranted);
    }
    let same = held.iter().find(|h| h.server == server);
    match same {
        Some(h) if !matches!(h.status, SessionStatus::Closed { .. }) => {
            Ok(OpenDecision::Reuse(h.session))
        }
        Some(h) => Ok(OpenDecision::Open {
            server,
            limits,
            replaces: Some(h.session),
        }),
        None => {
            let live = held
                .iter()
                .filter(|h| !matches!(h.status, SessionStatus::Closed { .. }))
                .count();
            if live >= APP_SESSION_MAX_SERVERS {
                return Err(Refusal::Limit);
            }
            Ok(OpenDecision::Open {
                server,
                limits,
                replaces: None,
            })
        }
    }
}

/// What a send may go over: the session's server CID, once it has signed in.
pub(crate) fn decide_send(
    session: Option<(&SessionStatus, u64, SessionLimits)>,
    body_len: usize,
) -> Result<u64, Refusal> {
    let (status, server_cid, limits) = session.ok_or(Refusal::Invalid)?;
    if !matches!(status, SessionStatus::Connected) {
        return Err(Refusal::Invalid);
    }
    let cap = (limits.max_msg_bytes as usize).min(APP_SESSION_MAX_BODY_BYTES);
    if body_len > cap {
        return Err(Refusal::Limit);
    }
    Ok(server_cid)
}

/// The agent-held account's handle on the external server: `<id_fp>.app-<install_id[..8]>`.
/// `None` for an install id whose first 8 characters a handle cannot carry.
pub(crate) fn app_handle(id_fp: &str, install_id: &str) -> Option<String> {
    let prefix = install_id.get(..8)?;
    let handle = format!("{id_fp}.app-{prefix}");
    citadel_identity::validate_handle(&handle).ok()?;
    Some(handle)
}

#[cfg(test)]
#[path = "rules_tests.rs"]
mod tests;

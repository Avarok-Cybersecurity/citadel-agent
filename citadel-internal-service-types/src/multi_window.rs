//! One session, several windows.
//!
//! A session can be attached to several localhost connections at once: tabs in
//! different browsers, a browser and the installed PWA. Every session-scoped
//! notification reaches every attached connection; a request's own response
//! reaches only the connection that sent it.
//!
//! The attached connections are ordered. The first is the session's
//! **primary**, the rest **secondary**; the role says where per-session work
//! that must happen once (see citadel-workspace
//! `docs/plans/multi-window-sessions.md`) is done.

use crate::plaintext_debug_fmt;
use custom_debug::Debug;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// What one attached connection is to a session.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum SessionRole {
    /// The longest-attached connection; does the once-per-session work.
    Primary,
    /// Attached after the primary; receives everything the session receives.
    Secondary,
    /// No longer attached: another window took the session over.
    Detached,
}

/// Sent to each attached connection whenever the set of attached connections
/// changes, and only then: a session nobody else ever joins never sends one,
/// so a UI that predates this never sees it from its own session.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct SessionRoleNotification {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub role: SessionRole,
    /// How many connections are attached now, this one included (0 if detached).
    pub attached: u32,
    pub request_id: Option<Uuid>,
}

/// What proves a caller may join a session another connection holds.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum AttachProof {
    /// The account's password; checked as a live sign-in checks it.
    Password(
        #[cfg_attr(feature = "typescript", ts(type = "number[]"))]
        #[debug(skip)]
        crate::SecBuffer,
    ),
    /// A token an earlier password attach to this same session returned.
    Token(
        #[cfg_attr(feature = "typescript", ts(type = "number[]"))]
        #[debug(with = plaintext_debug_fmt)]
        Vec<u8>,
    ),
}

/// `AttachSession` succeeded: this connection now receives the session.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct SessionAttached {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub role: SessionRole,
    /// Proves a later attach to this session without the password. Lives as
    /// long as the session does, in the agent's memory only.
    // A secret: length only, never a single byte of it (plaintext_debug_fmt).
    #[cfg_attr(feature = "typescript", ts(type = "number[]"))]
    #[debug(with = plaintext_debug_fmt)]
    pub token: Vec<u8>,
    pub request_id: Option<Uuid>,
}

/// What a connection's client can do, declared once per connection.
///
/// A client that declares `agent_ilm` never runs an ILM of its own for a
/// session: it sends reliable messages with `SendReliable` and the agent hosts
/// the session's ILM. A client that declares nothing is treated as one that
/// predates this (citadel-workspace `docs/plans/multi-window-sessions.md`).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ClientCapabilities {
    pub agent_ilm: bool,
}

/// The answer to `DeclareCapabilities`: what this agent offers.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AgentCapabilities {
    /// Always 0: capabilities belong to the agent, not to a session.
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    /// The agent hosts each session's ILM for a client that declared it.
    pub agent_ilm: bool,
    /// A session may be attached to several connections (`AttachSession`).
    pub multi_window: bool,
    /// The agent keeps this account's peers connected itself; see
    /// `ServiceConnectionAccepted::supervises_p2p`.
    #[serde(default)]
    pub supervises_p2p: bool,
    /// Something on the agent's side shows its native notices: the menu-bar app
    /// is subscribed. Kept current by `NoticesHeardNotification`. Absent from an
    /// older agent, which is what `false` means here.
    #[serde(default)]
    pub notices_heard: bool,
    pub request_id: Option<Uuid>,
}

/// A `SendReliable` was accepted by the session's ILM: it is stored and will be
/// retransmitted until the peer acknowledges it. Not "delivered".
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct SendReliableAccepted {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub peer_cid: u64,
    pub request_id: Option<Uuid>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Responses are logged whole at debug level; a token in the log is a
    /// credential in the log.
    #[test]
    fn a_token_never_prints_its_bytes() {
        let attached = SessionAttached {
            cid: 1,
            role: SessionRole::Secondary,
            token: vec![0xAB; 32],
            request_id: None,
        };
        let proof = AttachProof::Token(vec![0xAB; 32]);
        for printed in [format!("{attached:?}"), format!("{proof:?}")] {
            assert!(!printed.contains("171"), "token bytes in {printed}");
            assert!(
                !printed.to_lowercase().contains("ab, ab"),
                "token bytes in {printed}"
            );
        }
        let password = AttachProof::Password(crate::SecBuffer::from(b"hunter2".to_vec()));
        assert!(!format!("{password:?}").contains("hunter2"));
    }

    /// A browser reads the greeting to decide whether to declare. An older
    /// agent's greeting has no `agent_ilm`, and must read as "does not host",
    /// or the browser would wait on a declaration that agent never answers.
    #[test]
    fn an_older_agents_greeting_offers_no_hosting() {
        let older: crate::ServiceConnectionAccepted =
            serde_json::from_str(r#"{"cid":0,"request_id":null}"#).unwrap();
        assert!(!older.agent_ilm);
        let newer: crate::ServiceConnectionAccepted =
            serde_json::from_str(r#"{"cid":0,"request_id":null,"agent_ilm":true}"#).unwrap();
        assert!(newer.agent_ilm);
    }
}

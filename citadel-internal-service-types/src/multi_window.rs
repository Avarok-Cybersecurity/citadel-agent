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
}

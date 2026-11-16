//! The agent's portable identity on the UI wire (`ACCOUNT-IDENTITY-V2-PORTABLE.md` rev b §4,
//! contracts `identity-api.md`, `server-identity-challenge.md`).
//!
//! The identity seed never crosses this wire: the UI sends the unlock password and receives
//! public data (the identity's key, its fingerprint, bindings, a server's fingerprint). The one
//! exception is `IdentityOutcome::Exported`, the recovery phrase, which the agent gives only to
//! a request that re-proves the unlock password.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// What an account registered with the identity pinned at registration, as a window shows it.
/// `None` on an account registered with a typed password.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AccountIdentity {
    /// What the person typed and sees: the part of the username before `@`.
    pub handle: String,
    /// The server's identity, pinned at registration and never recomputed.
    pub canonical_domain: String,
    /// The fingerprint (16 base32 characters) of the identity that registered the account.
    pub id_fp: String,
    /// The fingerprint of the server key pinned for the account.
    pub server_fp: String,
    /// The pinned key has answered a challenge, or came from the server's signed
    /// `/.well-known/citadel`. `false` for a `Pinned` registration until the first challenge
    /// matches: the UI shows "unverified domain".
    pub verified: bool,
}

/// How a `RegisterIdentity` learns the server's key. No default: the person (or the server's
/// operator) chooses.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum IdentityDiscovery {
    /// Fetch `https://<canonical of server_addr>/.well-known/citadel`, verify it, dial its
    /// `citadel.wss` and register under its `canonical_domain`.
    WellKnown,
    /// A server without HTTPS (`[identity] discovery = "pinned"`): dial `server_addr` as typed,
    /// register under its canonical form, and pin the fingerprint the operator handed out
    /// (`abcd-efgh-ijkl-mnop`, dashes optional). Unverified until a challenge matches it.
    Pinned { server_fp: String },
}

/// Why an identity request was refused, for the UI to act on.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum IdentityRefusal {
    /// This agent was started without an identity store.
    NotConfigured,
    /// No identity exists yet: create or import one.
    NoIdentity,
    /// An identity exists; this agent holds one.
    AlreadyExists,
    /// The identity is locked: unlock it first.
    Locked,
    /// The unlock password does not open the sealed seed.
    WrongPassword,
    /// The recovery phrase is not 24 known words with a matching check.
    BadPhrase,
    /// The account was not registered with an identity.
    NotAnIdentityAccount,
    /// The stored identity or pin could not be read or written.
    Storage,
    /// A reason this build does not know. Never sent by the agent.
    #[serde(other)]
    Unknown,
}

/// The identity's state as the agent holds it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum IdentityState {
    None,
    Locked { id_fp: String },
    Unlocked { id_fp: String },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum IdentityOutcome {
    Status {
        state: IdentityState,
    },
    /// The identity's public key (base64url, 1952 bytes) and fingerprint. Created unlocked.
    Created {
        pk: String,
        id_fp: String,
    },
    Unlocked {
        id_fp: String,
    },
    Locked,
    /// The 24-word recovery phrase. Shown once; never logged.
    Exported {
        phrase: Vec<String>,
    },
    Imported {
        pk: String,
        id_fp: String,
    },
    /// The binding (base64url) for account `cid`, to publish as profile metadata.
    Binding {
        #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
        cid: u64,
        binding: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct IdentitySuccess {
    /// The account a `Binding` is for; 0 otherwise (the identity is the agent's, not a session's).
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub outcome: IdentityOutcome,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct IdentityFailure {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub reason: IdentityRefusal,
    pub message: String,
}

/// A server's verified `/.well-known/citadel`, summarised for the "add a server" screen.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ServerDiscoverySuccess {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub canonical_domain: String,
    pub citadel_wss: String,
    pub server_fp: String,
    pub former_domains: Vec<String>,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub issued_at: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ServerDiscoveryFailure {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub message: String,
}

/// The server a session reached did not prove the key pinned for its account: the agent
/// disconnected it and left the account untouched. Never a silent re-pin.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ServerIdentityMismatch {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub domain: String,
    pub pinned_fp: String,
    /// The fingerprint of the key the server answered with; empty when it gave none.
    pub offered_fp: String,
    pub message: String,
}

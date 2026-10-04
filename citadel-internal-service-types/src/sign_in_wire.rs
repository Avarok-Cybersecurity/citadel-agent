//! The agent's wire form of the SDK's sign-in management types (`citadel_types::auth`).
//!
//! The shapes are the SDK's, field for field, so the JSON is identical. They are the agent's own
//! so that they are generated with the rest of the agent's TypeScript (the published protocol
//! types package does not carry the SDK's yet) and so that they redact under `{:?}`: every
//! response is logged whole at debug level, and a credential id identifies a user's key.
//!
//! The conversions match every variant with no catch-all, so a variant the SDK adds is a compile
//! error here rather than a silent gap.

use crate::{plaintext_debug_fmt, RecoveryCodes};
use citadel_types::auth as sdk;
use custom_debug::Debug;
use serde::{Deserialize, Serialize};

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// Which factors a sign-in must prove (`citadel_types::auth::SignInPolicy`).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum SignInPolicy {
    Password,
    PasswordAndKey,
    KeyOnly,
}

/// What a factor is derived from (`citadel_types::auth::FactorKind`).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum FactorKind {
    Password,
    SecurityKey,
    RecoveryCode,
}

/// One enrolled factor (`citadel_types::auth::SignInCredential`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct SignInCredential {
    pub id: u32,
    pub kind: FactorKind,
    pub label: String,
    /// The WebAuthn credential id, for a security key.
    #[debug(with = optional_bytes_fmt)]
    pub credential_id: Option<Vec<u8>>,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub created_ms: u64,
    #[cfg_attr(feature = "typescript", ts(type = "bigint | null"))]
    pub last_used_ms: Option<u64>,
    /// A recovery code that has been used. It can never sign in again.
    pub consumed: bool,
}

/// A change to the account's sign-in factors (`citadel_types::auth::SignInManagementOp`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum SignInManagementOp {
    ListCredentials,
    AddSecurityKey {
        #[debug(with = plaintext_debug_fmt)]
        credential_id: Vec<u8>,
        label: String,
    },
    RenameCredential {
        id: u32,
        label: String,
    },
    RemoveCredential {
        id: u32,
    },
    SetSignInPolicy {
        policy: SignInPolicy,
    },
    RegenerateRecoveryCodes,
}

/// What a management change did (`citadel_types::auth::SignInManagementOutcome`). The new
/// recovery codes are [`RecoveryCodes`], which never print.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum SignInManagementOutcome {
    Credentials {
        policy: SignInPolicy,
        credentials: Vec<SignInCredential>,
    },
    Added {
        id: u32,
    },
    Renamed,
    Removed,
    PolicySet,
    RecoveryCodes(RecoveryCodes),
}

fn optional_bytes_fmt(val: &Option<Vec<u8>>, f: &mut std::fmt::Formatter) -> std::fmt::Result {
    match val {
        Some(bytes) => plaintext_debug_fmt(bytes, f),
        None => f.write_str("None"),
    }
}

impl From<SignInPolicy> for sdk::SignInPolicy {
    fn from(policy: SignInPolicy) -> Self {
        match policy {
            SignInPolicy::Password => Self::Password,
            SignInPolicy::PasswordAndKey => Self::PasswordAndKey,
            SignInPolicy::KeyOnly => Self::KeyOnly,
        }
    }
}

impl From<sdk::SignInPolicy> for SignInPolicy {
    fn from(policy: sdk::SignInPolicy) -> Self {
        match policy {
            sdk::SignInPolicy::Password => Self::Password,
            sdk::SignInPolicy::PasswordAndKey => Self::PasswordAndKey,
            sdk::SignInPolicy::KeyOnly => Self::KeyOnly,
        }
    }
}

impl From<sdk::FactorKind> for FactorKind {
    fn from(kind: sdk::FactorKind) -> Self {
        match kind {
            sdk::FactorKind::Password => Self::Password,
            sdk::FactorKind::SecurityKey => Self::SecurityKey,
            sdk::FactorKind::RecoveryCode => Self::RecoveryCode,
        }
    }
}

impl From<sdk::SignInCredential> for SignInCredential {
    fn from(c: sdk::SignInCredential) -> Self {
        Self {
            id: c.id,
            kind: c.kind.into(),
            label: c.label,
            credential_id: c.credential_id,
            created_ms: c.created_ms,
            last_used_ms: c.last_used_ms,
            consumed: c.consumed,
        }
    }
}

impl From<SignInManagementOp> for sdk::SignInManagementOp {
    fn from(op: SignInManagementOp) -> Self {
        match op {
            SignInManagementOp::ListCredentials => Self::ListCredentials,
            SignInManagementOp::AddSecurityKey {
                credential_id,
                label,
            } => Self::AddSecurityKey {
                credential_id,
                label,
            },
            SignInManagementOp::RenameCredential { id, label } => {
                Self::RenameCredential { id, label }
            }
            SignInManagementOp::RemoveCredential { id } => Self::RemoveCredential { id },
            SignInManagementOp::SetSignInPolicy { policy } => Self::SetSignInPolicy {
                policy: policy.into(),
            },
            SignInManagementOp::RegenerateRecoveryCodes => Self::RegenerateRecoveryCodes,
        }
    }
}

impl From<sdk::SignInManagementOutcome> for SignInManagementOutcome {
    fn from(outcome: sdk::SignInManagementOutcome) -> Self {
        match outcome {
            sdk::SignInManagementOutcome::Credentials {
                policy,
                credentials,
            } => Self::Credentials {
                policy: policy.into(),
                credentials: credentials.into_iter().map(Into::into).collect(),
            },
            sdk::SignInManagementOutcome::Added { id } => Self::Added { id },
            sdk::SignInManagementOutcome::Renamed => Self::Renamed,
            sdk::SignInManagementOutcome::Removed => Self::Removed,
            sdk::SignInManagementOutcome::PolicySet => Self::PolicySet,
            sdk::SignInManagementOutcome::RecoveryCodes(codes) => {
                Self::RecoveryCodes(RecoveryCodes(codes))
            }
        }
    }
}

#[cfg(test)]
#[path = "sign_in_wire_tests_more.rs"]
mod tests;

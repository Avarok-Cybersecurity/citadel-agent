//! The agent's wire form of the SDK's sign-in management types (`citadel_types::auth`).
//!
//! The shapes are the SDK's, field for field, so the JSON is identical. They are the agent's own
//! so that they are generated with the rest of the agent's TypeScript (the published protocol
//! types package does not carry the SDK's yet) and so that they redact under `{:?}`: every
//! response is logged whole at debug level, and a credential id identifies a user's key.
//!
//! The conversions match every variant with no catch-all, so a variant the SDK adds is a compile
//! error here rather than a silent gap. The SDK's retired recovery-code slots are uninhabited:
//! the SDK refuses them when it decodes, so they never reach a conversion.

use crate::{plaintext_debug_fmt, RecoveryCodes};
use citadel_types::auth as sdk;
use citadel_types::crypto::SecBuffer;
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
    RecoveryPhrase,
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
    /// The SDK's flag for a used single-use factor; no factor the agent offers is single-use.
    pub consumed: bool,
}

/// A change to the account's sign-in factors (`citadel_types::auth::SignInManagementOp`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum SignInManagementOp {
    ListCredentials,
    /// The SDK asks a window to create the key's credential (a `Register` ceremony) once the
    /// server has named the relying party and the keys the account already has.
    AddSecurityKey {
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
    /// Set the account's password: a forgotten one from the recovery console (no step-up), or a
    /// change from a full session (the step-up holds the current password). The SDK turns the
    /// new password into a fresh factor on this device; the server never sees it.
    SetPassword {
        #[cfg_attr(feature = "typescript", ts(type = "number[]"))]
        new_password: SecBuffer,
    },
    /// Replace the recovery phrase with a new one. The agent binds it to the canonical domain of
    /// the account's server (`kernel/recovery_domain.rs`), so a window never names one. The new
    /// phrase's seed becomes the root of the account's RE-VFS key: every object the account
    /// stored is sealed again under it (`kernel/revfs_seed/rekey.rs`).
    RegenerateRecoveryPhrase,
}

/// What a management change did (`citadel_types::auth::SignInManagementOutcome`).
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
    PasswordSet,
    /// The new recovery phrase's words, which never print either.
    RecoveryPhrase(RecoveryCodes),
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
            sdk::FactorKind::RetiredRecoveryCode(retired) => match retired {},
            sdk::FactorKind::RecoveryPhrase => Self::RecoveryPhrase,
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

impl SignInManagementOp {
    /// The SDK's change. `canonical_domain` is read by `RegenerateRecoveryPhrase` alone; the
    /// new password of `SetPassword` is not part of the SDK's op (it goes to `set_password`).
    pub fn to_sdk(&self, canonical_domain: &str) -> sdk::SignInManagementOp {
        match self.clone() {
            Self::ListCredentials => sdk::SignInManagementOp::ListCredentials,
            Self::AddSecurityKey { label } => sdk::SignInManagementOp::AddSecurityKey { label },
            Self::RenameCredential { id, label } => {
                sdk::SignInManagementOp::RenameCredential { id, label }
            }
            Self::RemoveCredential { id } => sdk::SignInManagementOp::RemoveCredential { id },
            Self::SetSignInPolicy { policy } => sdk::SignInManagementOp::SetSignInPolicy {
                policy: policy.into(),
            },
            Self::SetPassword { .. } => sdk::SignInManagementOp::SetPassword,
            Self::RegenerateRecoveryPhrase => sdk::SignInManagementOp::RegenerateRecoveryPhrase {
                canonical_domain: canonical_domain.to_string(),
            },
        }
    }

    /// Whether a recovery console session may make this change: the SDK's rule, which reads the
    /// kind of change only, so no domain is needed to ask it.
    pub fn allowed_in_recovery(&self) -> bool {
        self.to_sdk("").allowed_in_recovery()
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
            sdk::SignInManagementOutcome::RetiredRecoveryCodes(retired) => match retired {},
            sdk::SignInManagementOutcome::PasswordSet => Self::PasswordSet,
            sdk::SignInManagementOutcome::RecoveryPhrase(words) => {
                Self::RecoveryPhrase(RecoveryCodes(words))
            }
        }
    }
}

#[cfg(test)]
#[path = "sign_in_wire_tests_more.rs"]
mod tests;

//! Post-quantum sign-in at the agent's boundary: the recovery codes a registration returns, the
//! security-key challenge the agent relays to a window, and the management of an account's
//! sign-in factors.
//!
//! The account-level types (`SignInPolicy`, `SignInManagementOp`, ...) are in
//! `sign_in_wire.rs`: the SDK's `citadel_types::auth` shapes, in the agent's wire.
//!
//! Three things here are secret: recovery codes, a PRF output and a step-up password. None of
//! them is ever printed: the PRF output and the password are `SecBuffer`s, and the codes are
//! [`RecoveryCodes`], whose `Debug` shows only how many there are.

use crate::{plaintext_debug_fmt, SignInManagementOutcome};
use citadel_types::crypto::SecBuffer;
use custom_debug::Debug;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroize;

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// Recovery codes, formatted for display. Shown to the user once; never logged and never
/// stored by the agent, and wiped from memory when dropped.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
#[serde(transparent)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct RecoveryCodes(pub Vec<String>);

impl std::fmt::Debug for RecoveryCodes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RecoveryCodes(<{} redacted>)", self.0.len())
    }
}

impl Drop for RecoveryCodes {
    fn drop(&mut self) {
        self.0.iter_mut().for_each(Zeroize::zeroize);
    }
}

/// A failure the UI can act on, beside the message it shows.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum FailureReason {
    /// The server needs an admission token (show the Turnstile widget) and none was sent.
    AdmissionRequired,
    /// The server refused the admission token that was sent (reset the widget and retry).
    AdmissionFailed,
}

/// The factors a window offers to prove the account again before a change to its sign-in
/// factors. A session signed in with a recovery code offers none.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct StepUp {
    #[cfg_attr(feature = "typescript", ts(type = "number[] | null"))]
    pub password: Option<SecBuffer>,
    /// Whether the window can answer a [`SecurityKeyChallengeNotification`]. Adding a key
    /// needs it (the new key's touch) even when the step-up itself is the password alone.
    pub security_key: bool,
}

/// Why the agent is asking for a touch.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum SecurityKeyPurpose {
    SignIn,
    /// A fresh proof of the account before a change to its factors.
    StepUp,
    /// The key being added: its PRF output becomes the new factor.
    Enrol,
}

/// The SDK needs a security-key touch. The window runs WebAuthn `get` with
/// `allowCredentials = allowed_credential_ids` and the PRF extension evaluated at `prf_salt`,
/// then answers with `SecurityKeyAnswer` (or `SecurityKeyDecline`).
///
/// `request_id` is the request of the window that asked (its `Connect` or
/// `SignInManagement`). `cid` is the session's, or 0 for a sign-in, which has none yet.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct SecurityKeyChallengeNotification {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    /// Names this challenge in the answer.
    pub challenge_id: Uuid,
    pub purpose: SecurityKeyPurpose,
    #[debug(with = credential_ids_fmt)]
    pub allowed_credential_ids: Vec<Vec<u8>>,
    /// 32 bytes: the PRF `eval.first` input.
    #[debug(with = plaintext_debug_fmt)]
    pub prf_salt: Vec<u8>,
    /// How long the SDK waits for the touch. An answer after that is refused.
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub expires_in_ms: u64,
}

/// The agent handed the answer to the SDK. Whether it signs in is the server's to decide, and
/// the outcome is the asking request's own response.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct SecurityKeyAnswerSuccess {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub challenge_id: Uuid,
}

/// The answer was not used: the challenge is unknown, already answered or expired, the window
/// was not asked, the credential was not one of the allowed ones, or the PRF output is not 32
/// bytes. A refused answer leaves the challenge open for a valid one.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct SecurityKeyAnswerFailure {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub challenge_id: Uuid,
    pub message: String,
}

/// What a `SignInManagement` request did. For `RegenerateRecoveryCodes` the outcome holds the
/// new codes, which is the only time they can be shown.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct SignInManagementSuccess {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub outcome: SignInManagementOutcome,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct SignInManagementFailure {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub message: String,
}

/// How many credentials a challenge allows, never which.
#[allow(clippy::ptr_arg)] // custom_debug hands the field over as `&Vec`.
fn credential_ids_fmt(ids: &Vec<Vec<u8>>, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{{{} credential id(s), redacted}}", ids.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "7H3Q-K2M9-PX4D-W8RT";

    #[test]
    fn recovery_codes_never_reach_a_debug_string() {
        let codes = RecoveryCodes(vec![CODE.to_string(); 10]);
        let printed = format!("{codes:?}");
        assert!(!printed.contains(CODE), "{printed}");
        assert!(printed.contains("10"), "{printed}");
    }

    #[test]
    fn regenerated_codes_never_reach_a_debug_string() {
        let success = SignInManagementSuccess {
            cid: 1,
            request_id: None,
            outcome: SignInManagementOutcome::RecoveryCodes(RecoveryCodes(vec![CODE.to_string()])),
        };
        let printed = format!("{success:?}");
        assert!(!printed.contains(CODE), "{printed}");
    }

    #[test]
    fn other_outcomes_still_print() {
        let success = SignInManagementSuccess {
            cid: 1,
            request_id: None,
            outcome: SignInManagementOutcome::Added { id: 4 },
        };
        assert!(format!("{success:?}").contains("Added"));
    }

    #[test]
    fn recovery_codes_cross_the_wire_as_a_plain_list() {
        let codes = RecoveryCodes(vec![CODE.to_string()]);
        let json = serde_json::to_string(&codes).unwrap();
        assert_eq!(json, format!("[\"{CODE}\"]"));
    }
}

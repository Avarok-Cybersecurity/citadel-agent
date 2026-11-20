//! Post-quantum sign-in at the agent's boundary: the recovery phrase a registration returns, the
//! security-key challenge the agent relays to a window, and the management of an account's
//! sign-in factors.
//!
//! The account-level types (`SignInPolicy`, `SignInManagementOp`, ...) are in
//! `sign_in_wire.rs`: the SDK's `citadel_types::auth` shapes, in the agent's wire.
//!
//! Three things here are secret: the recovery phrase, a PRF output and a step-up password. None
//! of them is ever printed: the PRF output and the password are `SecBuffer`s, and the phrase's
//! words are [`RecoveryCodes`], whose `Debug` shows only how many there are.

use crate::{plaintext_debug_fmt, SecurityKeyCeremony, SignInManagementOutcome};
use citadel_types::crypto::SecBuffer;
use custom_debug::Debug;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroize;

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// A recovery phrase's words, formatted for display. Shown to the user once; never logged and
/// never stored by the agent as words (only the seed they encode, sealed: kernel/revfs_seed),
/// and wiped from memory when dropped.
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
    /// A Connect named a server, and this agent's account with that username is registered
    /// to a different one. Nothing was dialled: signing in would have reached that other server.
    AccountOnAnotherServer,
    /// A Connect named a username this agent holds no account for.
    NoSuchAccount,
    /// A Register named a username this agent already holds an account for on a different
    /// server (`RegisterFailure::held_for_server`). One agent holds one account per username
    /// (the SDK stores accounts by `username_to_cid`), so registering would overwrite it.
    UsernameHeldForAnotherServer,
    /// A `RegisterIdentity` named a username this agent already holds, on any server (a
    /// re-created server included): registering would overwrite the account here. The UI
    /// offers to forget the old account instead.
    UsernameHeld,
    /// `RegisterIdentity` / `ConnectIdentity` need the identity unlocked.
    IdentityLocked,
    /// `ConnectIdentity` named an account with no identity pin, or one another identity holds.
    NotAnIdentityAccount,
    /// The server did not prove the key pinned for the account; the session was disconnected.
    ServerIdentityMismatch,
    /// An `AddSecurityKey` the SDK refused before sending: the server is too old to enrol a key
    /// whose credential the client creates (protocol 0.12.2), or has not enabled security keys.
    /// The UI says the server must be updated before a key can be added.
    ServerCannotEnrolSecurityKeys,
    /// `/.well-known/citadel` could not be fetched or did not verify.
    ServerDiscoveryFailed,
    /// The server's verified `/.well-known/citadel` has no `citadel` member: an issuer-only
    /// server (work.avarok.net) that runs no node, so there is nothing to register on.
    NoCitadelEndpoint,
    /// A reason this build does not know: a newer agent's. Decoded rather than refused, so
    /// the failure it rides on still reaches the page (as "update the page"); an unknown
    /// variant otherwise failed the whole response, and the page waited out a timeout.
    /// Never sent by the agent.
    #[serde(other)]
    Unknown,
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
    /// The key being added: the window creates its credential, whose PRF output becomes the
    /// new factor.
    Enrol,
}

/// The SDK needs a security-key touch. The window runs `ceremony` for relying party `rp_id`, with
/// the PRF extension evaluated at `prf_salt`, then answers with `SecurityKeyAnswer` (or
/// `SecurityKeyDecline`). The server names `rp_id`, so every client asks its keys under one. A
/// server at protocol 0.12.2 names none: the window then uses its own, as every window did for
/// 0.12.2. A `Register` ceremony always has one.
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
    pub rp_id: Option<String>,
    pub ceremony: SecurityKeyCeremony,
    /// 32 bytes: the PRF `eval.first` input.
    #[debug(with = plaintext_debug_fmt)]
    pub prf_salt: Vec<u8>,
    /// How long the SDK waits for the touch. An answer after that is refused.
    ///
    /// `bigint`, as every u64 is: the WASM client serializes with
    /// `serialize_large_number_types_as_bigints`, so a page receives a BigInt here, and a
    /// `number` type let it do arithmetic that threw (the touch prompt crashed in render).
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
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

/// What a `SignInManagement` request did. For `RegenerateRecoveryPhrase` the outcome holds the
/// new phrase, which is the only time it can be shown.
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
    /// Why, when the UI can say more than the message (`ServerCannotEnrolSecurityKeys`).
    pub reason: Option<FailureReason>,
}

/// A `RecoverAccount` recovered the account on this device: every other device's bundle was
/// revoked, and those devices are signed out. Sent before the phrase sign-in's own answer
/// (`ConnectSuccess`, a recovery console session, or `ConnectFailure`), under the same request.
/// Not sent when this device already held the account: then nothing was revoked.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct RecoverAccountSuccess {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
}

/// The recovery was refused. A wrong phrase, an unknown username and a throttled attempt read
/// alike (the server does not say which); a phrase that is not 24 words of the list, or a server
/// with no canonical domain, is refused before anything is dialled.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct RecoverAccountFailure {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub message: String,
    /// `AdmissionRequired` / `AdmissionFailed` when the server's admission check refused it.
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub reason_code: Option<FailureReason>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub admission_challenge: Option<crate::AdmissionChallengeWire>,
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
    fn a_regenerated_phrase_never_reaches_a_debug_string() {
        let success = SignInManagementSuccess {
            cid: 1,
            request_id: None,
            outcome: SignInManagementOutcome::RecoveryPhrase(RecoveryCodes(vec![CODE.to_string()])),
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
    fn a_registrations_phrase_never_reaches_a_debug_string() {
        let phrase = RecoveryCodes(vec!["zebra".to_string(); 24]);
        let printed = format!("{phrase:?}");
        assert!(!printed.contains("zebra"), "{printed}");
        assert!(printed.contains("24"), "{printed}");
    }

    #[test]
    fn recovery_codes_cross_the_wire_as_a_plain_list() {
        let codes = RecoveryCodes(vec![CODE.to_string()]);
        let json = serde_json::to_string(&codes).unwrap();
        assert_eq!(json, format!("[\"{CODE}\"]"));
    }
}

//! What a change a session makes to its own account's sign-in does to how the session can be
//! opened again without its user. Pure.
//!
//! A session keeps what opened it (`Reauth`) for the agent's reconnect. The step after
//! registration offers a security key and a policy that asks for it; the session that made
//! that change had signed in with the password alone and went on keeping it, so its next
//! reconnect offered a password the account no longer signs in with: the server answered
//! "Authentication failed" to every attempt. A changed password stranded it the same way.

use super::KEY_REAUTH;
use crate::kernel::reconnect::Reauth;
use citadel_internal_service_types::{SignInManagementOp, SignInPolicy};
use citadel_sdk::prelude::{ResumeCredential, SecBuffer};

/// The part of a management change that decides how the account signs in from now on.
pub(crate) enum Change {
    Policy(SignInPolicy),
    Password(SecBuffer),
}

impl Change {
    /// `op`'s change to how the account signs in, or `None` for one that makes none.
    ///
    /// Adding or removing a credential is none: neither changes the policy, and the server
    /// refuses a removal that would leave the policy without a factor it asks for. So under
    /// the password policy a key was never asked for, added or removed, and the password
    /// cannot be removed; under a key policy the session already needs its user.
    pub(crate) fn of(op: &SignInManagementOp) -> Option<Self> {
        match op {
            SignInManagementOp::SetSignInPolicy { policy } => Some(Self::Policy(*policy)),
            SignInManagementOp::SetPassword { new_password } => {
                Some(Self::Password(new_password.clone()))
            }
            SignInManagementOp::ListCredentials
            | SignInManagementOp::AddSecurityKey { .. }
            | SignInManagementOp::RenameCredential { .. }
            | SignInManagementOp::RemoveCredential { .. }
            | SignInManagementOp::RegenerateRecoveryPhrase => None,
        }
    }
}

/// How the session is opened again once the server has accepted `change`.
///
/// A session that already needs its user goes on needing them: a policy set back to the
/// password alone does not hand the agent a password it was never given.
pub(crate) fn after(current: Reauth, change: Change) -> Reauth {
    match (current, change) {
        (Reauth::NeedsUser(reason), _) => Reauth::NeedsUser(reason),
        // The server re-binds a credential the change was made with (requests/
        // sign_in_management.rs puts the new one here); one it did not re-bind is refused by
        // name at the next reconnect, which says more than giving it up here would.
        (Reauth::Resume(credential), _) => Reauth::Resume(credential),
        (Reauth::Password(password), Change::Policy(SignInPolicy::Password)) => {
            Reauth::Password(password)
        }
        (
            Reauth::Password(_),
            Change::Policy(SignInPolicy::PasswordAndKey | SignInPolicy::KeyOnly),
        ) => Reauth::NeedsUser(KEY_REAUTH),
        (Reauth::Password(_), Change::Password(new_password)) => Reauth::Password(new_password),
    }
}

/// What a session keeps once the server has accepted a change it made. A credential the
/// server re-bound takes the place of the one the session held; nothing else is given one.
pub(crate) fn kept_after(
    current: Reauth,
    change: Option<Change>,
    reissued: Option<ResumeCredential>,
) -> Reauth {
    match (current, reissued, change) {
        (Reauth::Resume(_), Some(credential), _) => Reauth::Resume(credential),
        (current, _, Some(change)) => after(current, change),
        (current, _, None) => current,
    }
}

/// What a change does to the fingerprint a window proves the password against.
pub(crate) enum Fingerprint {
    Unchanged,
    /// The password no longer signs the account in by itself.
    None,
    /// The password that now does.
    Of(SecBuffer),
}

/// The fingerprint of a session that signed in again with `before`, once `change` is made. A
/// session that keeps its password is fingerprinted from the one it keeps; one that keeps a
/// resume credential has no password, so its fingerprint follows the change itself.
pub(crate) fn fingerprint_after(before: &Reauth, change: Option<&Change>) -> Fingerprint {
    let asks_for_a_key = |policy: &SignInPolicy| {
        matches!(policy, SignInPolicy::PasswordAndKey | SignInPolicy::KeyOnly)
    };
    match (before, change) {
        (_, None) | (Reauth::NeedsUser(_), _) => Fingerprint::Unchanged,
        (_, Some(Change::Password(new_password))) => Fingerprint::Of(new_password.clone()),
        (_, Some(Change::Policy(policy))) if asks_for_a_key(policy) => Fingerprint::None,
        (_, Some(Change::Policy(_))) => Fingerprint::Unchanged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::sign_in::RECOVERY_REAUTH;

    fn kept(reauth: &Reauth) -> Option<&[u8]> {
        match reauth {
            Reauth::Password(password) => Some(password.as_ref()),
            Reauth::Resume(_) | Reauth::NeedsUser(_) => None,
        }
    }

    fn needs(reauth: &Reauth) -> Option<&'static str> {
        match reauth {
            Reauth::Password(_) | Reauth::Resume(_) => None,
            Reauth::NeedsUser(reason) => Some(reason),
        }
    }

    fn password() -> Reauth {
        Reauth::Password(SecBuffer::from("first"))
    }

    #[test]
    fn a_policy_that_asks_for_a_key_ends_the_password_reconnect() {
        for policy in [SignInPolicy::PasswordAndKey, SignInPolicy::KeyOnly] {
            let next = after(password(), Change::Policy(policy));
            assert_eq!(needs(&next), Some(KEY_REAUTH), "{policy:?}");
        }
    }

    /// Negative control: the policy the session already satisfies changes nothing.
    #[test]
    fn the_password_policy_keeps_the_password() {
        let next = after(password(), Change::Policy(SignInPolicy::Password));
        assert_eq!(kept(&next), Some(&b"first"[..]));
    }

    #[test]
    fn a_new_password_replaces_the_kept_one() {
        let next = after(password(), Change::Password(SecBuffer::from("second")));
        assert_eq!(kept(&next), Some(&b"second"[..]));
    }

    #[test]
    fn a_session_that_needs_its_user_still_does_after_any_change() {
        for reason in [KEY_REAUTH, RECOVERY_REAUTH] {
            let changes = [
                Change::Policy(SignInPolicy::Password),
                Change::Policy(SignInPolicy::KeyOnly),
                Change::Password(SecBuffer::from("second")),
            ];
            for change in changes {
                let next = after(Reauth::NeedsUser(reason), change);
                assert_eq!(needs(&next), Some(reason));
            }
        }
    }

    #[test]
    fn only_a_policy_or_a_password_changes_how_the_account_signs_in() {
        let none = [
            SignInManagementOp::ListCredentials,
            SignInManagementOp::AddSecurityKey { label: "k".into() },
            SignInManagementOp::RenameCredential {
                id: 1,
                label: "k".into(),
            },
            SignInManagementOp::RemoveCredential { id: 1 },
            SignInManagementOp::RegenerateRecoveryPhrase,
        ];
        assert!(none.iter().all(|op| Change::of(op).is_none()));
        let policy = SignInManagementOp::SetSignInPolicy {
            policy: SignInPolicy::KeyOnly,
        };
        assert!(matches!(
            Change::of(&policy),
            Some(Change::Policy(SignInPolicy::KeyOnly))
        ));
    }
}

//! A session holding a resume credential, across a change it makes to its own account.

use super::reauth_change::{fingerprint_after, kept_after, Change, Fingerprint};
use crate::kernel::reconnect::Reauth;
use crate::kernel::resume::test_support::credential;
use citadel_internal_service_types::SignInPolicy;
use citadel_sdk::prelude::SecBuffer;

fn changes() -> [Change; 3] {
    [
        Change::Policy(SignInPolicy::Password),
        Change::Policy(SignInPolicy::PasswordAndKey),
        Change::Password(SecBuffer::from("second")),
    ]
}

/// Control: keep the old credential when the server re-bound it.
#[tokio::test]
async fn the_credential_the_server_re_bound_replaces_the_one_held() {
    let (old, new) = (credential(3_600).await, credential(7_200).await);
    for change in changes().map(Some).into_iter().chain([None]) {
        let kept = kept_after(Reauth::Resume(old.clone()), change, Some(new.clone()));
        let Reauth::Resume(kept) = kept else {
            panic!("the session gave up its credential");
        };
        assert_eq!(kept.valid_until_utc(), new.valid_until_utc());
    }
}

/// Control: turn a held credential into `NeedsUser` on a key policy.
#[tokio::test]
async fn a_credential_the_server_did_not_re_bind_is_kept_for_the_server_to_refuse_by_name() {
    let held = credential(3_600).await;
    for change in changes() {
        let kept = kept_after(Reauth::Resume(held.clone()), Some(change), None);
        assert!(matches!(kept, Reauth::Resume(_)));
    }
}

/// Control: give a password session the credential.
#[tokio::test]
async fn a_session_that_held_no_credential_is_not_given_one() {
    let stray = credential(3_600).await;
    let password = || Reauth::Password(SecBuffer::from("first"));
    assert!(matches!(
        kept_after(password(), None, Some(stray.clone())),
        Reauth::Password(_)
    ));
    let key_policy = Some(Change::Policy(SignInPolicy::KeyOnly));
    assert!(matches!(
        kept_after(password(), key_policy, Some(stray)),
        Reauth::NeedsUser(_)
    ));
}

/// Control: answer `Unchanged` for a new password.
#[tokio::test]
async fn the_fingerprint_follows_the_change_for_a_session_with_no_password_kept() {
    let held = Reauth::Resume(credential(3_600).await);
    let password = Reauth::Password(SecBuffer::from("first"));
    for before in [&held, &password] {
        let new = Change::Password(SecBuffer::from("second"));
        let Fingerprint::Of(of) = fingerprint_after(before, Some(&new)) else {
            panic!("a new password is the new fingerprint");
        };
        assert_eq!(of.as_ref(), b"second");
        for policy in [SignInPolicy::PasswordAndKey, SignInPolicy::KeyOnly] {
            let key = Change::Policy(policy);
            assert!(matches!(
                fingerprint_after(before, Some(&key)),
                Fingerprint::None
            ));
        }
        let same = Change::Policy(SignInPolicy::Password);
        assert!(matches!(
            fingerprint_after(before, Some(&same)),
            Fingerprint::Unchanged
        ));
        assert!(matches!(
            fingerprint_after(before, None),
            Fingerprint::Unchanged
        ));
    }
    let needs = Reauth::NeedsUser("sign in again");
    let new = Change::Password(SecBuffer::from("second"));
    assert!(matches!(
        fingerprint_after(&needs, Some(&new)),
        Fingerprint::Unchanged
    ));
}

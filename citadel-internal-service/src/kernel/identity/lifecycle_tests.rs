//! The identity's lifecycle against an in-memory store: created unlocked, locked, refused on a
//! wrong password, opened on the right one, exported, and recreated from its phrase elsewhere.
//!
//! Control: turn a failed unseal in `open` into an all-zero seed (any password opens) →
//! `a_wrong_password_never_opens_the_seed` goes red.

use super::*;
use crate::kernel::identity::test_ports::host;
use citadel_internal_service_types::{IdentityOutcome, IdentityRefusal, IdentityState};

fn secret(text: &str) -> Zeroizing<Vec<u8>> {
    Zeroizing::new(text.as_bytes().to_vec())
}

#[tokio::test]
async fn an_identity_is_created_unlocked_and_a_second_is_refused() {
    let host = host();
    assert_eq!(
        host.status().await.unwrap(),
        IdentityOutcome::Status {
            state: IdentityState::None
        }
    );
    let IdentityOutcome::Created { id_fp, .. } = host.create(secret("unlock me")).await.unwrap()
    else {
        panic!("not created")
    };
    assert_eq!(
        host.status().await.unwrap(),
        IdentityOutcome::Status {
            state: IdentityState::Unlocked {
                id_fp: id_fp.clone()
            }
        }
    );
    let again = host.create(secret("unlock me")).await.unwrap_err();
    assert_eq!(again.0, IdentityRefusal::AlreadyExists);
    host.unlocked.lock();
    assert_eq!(
        host.status().await.unwrap(),
        IdentityOutcome::Status {
            state: IdentityState::Locked { id_fp }
        }
    );
}

#[tokio::test]
async fn a_wrong_password_never_opens_the_seed() {
    let host = host();
    host.create(secret("unlock me")).await.unwrap();
    host.unlocked.lock();
    let refused = host.unlock(secret("not it")).await.unwrap_err();
    assert_eq!(refused.0, IdentityRefusal::WrongPassword);
    assert!(host.unlocked.current().is_none());
    assert_eq!(
        host.export(secret("not it")).await.unwrap_err().0,
        IdentityRefusal::WrongPassword
    );
    host.unlock(secret("unlock me")).await.unwrap();
    assert!(host.unlocked.current().is_some());
}

#[tokio::test]
async fn the_phrase_recreates_the_same_identity_on_another_agent() {
    let first = host();
    let IdentityOutcome::Created { id_fp, pk } = first.create(secret("one")).await.unwrap() else {
        panic!("not created")
    };
    let IdentityOutcome::Exported { phrase } = first.export(secret("one")).await.unwrap() else {
        panic!("not exported")
    };
    assert_eq!(phrase.len(), 24);
    let second = host();
    let imported = second
        .import(secret(&phrase.join(" ")), secret("two"))
        .await
        .unwrap();
    assert_eq!(imported, IdentityOutcome::Imported { pk, id_fp });
    let mut wrong = phrase.clone();
    wrong.swap(0, 1);
    let third = host();
    let refused = third
        .import(secret(&wrong.join(" ")), secret("x"))
        .await
        .unwrap_err();
    assert_eq!(refused.0, IdentityRefusal::BadPhrase);
}

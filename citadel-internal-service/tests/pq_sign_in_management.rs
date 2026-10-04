//! Managing an account's sign-in factors through the agent: every change is proven with a
//! step-up, and no change may leave the account unable to sign in.

use citadel_internal_service_test_common::pq::{spawn_agent, spawn_server, Server, PASSWORD};
use citadel_internal_service_test_common::pq_accounts::{add_key, password_offer};
use citadel_internal_service_test_common::pq_window::{FakeKey, Window};
use citadel_internal_service_test_common::setup_log;
use citadel_sdk::prelude::{
    FactorKind, SignInCredential, SignInManagementOp, SignInManagementOutcome, SignInPolicy,
};
use std::error::Error;

/// The listed factors and the policy the list reports.
fn credentials(outcome: SignInManagementOutcome) -> (SignInPolicy, Vec<SignInCredential>) {
    match outcome {
        SignInManagementOutcome::Credentials {
            policy,
            credentials,
        } => (policy, credentials),
        other => panic!("not a credential list: {other:?}"),
    }
}

fn of_kind(list: &[SignInCredential], kind: FactorKind) -> Vec<&SignInCredential> {
    list.iter().filter(|c| c.kind == kind).collect()
}

#[tokio::test]
async fn each_management_change_works_and_the_last_factor_cannot_be_removed(
) -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_server(Server::PostQuantum);
    let mut w = Window::open(spawn_agent().await?).await?;
    let user = citadel_internal_service_test_common::pq::username("mgmt");
    w.register(server, &user, PASSWORD).await??;
    let cid = w.sign_in(&user, &password_offer()).await??;
    let pw = Some(PASSWORD);
    let key = FakeKey::new(4);

    let (policy, list) = credentials(
        w.manage(cid, SignInManagementOp::ListCredentials, pw, None)
            .await??,
    );
    assert_eq!(policy, SignInPolicy::Password);
    assert_eq!(of_kind(&list, FactorKind::Password).len(), 1);
    assert_eq!(of_kind(&list, FactorKind::RecoveryCode).len(), 10);
    let password_id = of_kind(&list, FactorKind::Password)[0].id;

    // The password is the only way in: removing it is refused.
    let remove_password = SignInManagementOp::RemoveCredential { id: password_id };
    let refused = w.manage(cid, remove_password.clone(), pw, None).await?;
    assert!(
        refused.is_err(),
        "the last sign-in factor was removed: {refused:?}"
    );

    // A step-up with the wrong password changes nothing.
    let wrong = w.manage(cid, add_key(), Some("wrong"), Some(&key)).await?;
    assert!(wrong.is_err(), "a wrong step-up added a key: {wrong:?}");

    let SignInManagementOutcome::Added { id: key_id } =
        w.manage(cid, add_key(), pw, Some(&key)).await??
    else {
        panic!("the key was not added");
    };
    let rename = SignInManagementOp::RenameCredential {
        id: key_id,
        label: "Desk key".to_string(),
    };
    assert_eq!(
        w.manage(cid, rename, pw, None).await??,
        SignInManagementOutcome::Renamed
    );
    let (_, list) = credentials(
        w.manage(cid, SignInManagementOp::ListCredentials, pw, None)
            .await??,
    );
    let keys = of_kind(&list, FactorKind::SecurityKey);
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].label, "Desk key");

    // Key-only: now the key is the only way in, and it cannot be removed either.
    let key_only = SignInManagementOp::SetSignInPolicy {
        policy: SignInPolicy::KeyOnly,
    };
    let set = w.manage(cid, key_only, pw, Some(&key)).await??;
    assert_eq!(set, SignInManagementOutcome::PolicySet);
    let listed = w.manage(cid, SignInManagementOp::ListCredentials, None, Some(&key));
    let (policy, _) = credentials(listed.await??);
    assert_eq!(
        policy,
        SignInPolicy::KeyOnly,
        "the list does not report the policy"
    );
    let remove_key = SignInManagementOp::RemoveCredential { id: key_id };
    let refused = w.manage(cid, remove_key.clone(), None, Some(&key)).await?;
    assert!(
        refused.is_err(),
        "a key-only account's only key was removed"
    );

    // Back to the password, and then the key may go.
    let password_policy = SignInManagementOp::SetSignInPolicy {
        policy: SignInPolicy::Password,
    };
    let set = w.manage(cid, password_policy, None, Some(&key)).await??;
    assert_eq!(set, SignInManagementOutcome::PolicySet);
    assert_eq!(
        w.manage(cid, remove_key, pw, None).await??,
        SignInManagementOutcome::Removed
    );

    let SignInManagementOutcome::RecoveryCodes(codes) = w
        .manage(cid, SignInManagementOp::RegenerateRecoveryCodes, pw, None)
        .await??
    else {
        panic!("no new recovery codes");
    };
    assert_eq!(codes.len(), 10);
    Ok(())
}

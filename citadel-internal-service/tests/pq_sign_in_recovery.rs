//! The recovery phrase signs in to the recovery console: a session that may manage how the
//! account signs in (list, add a key, remove one, set the policy, set the password, replace the
//! phrase) and sign out, and nothing else. The agent answers everything else with a clear refusal
//! rather than leaving it to the server, which drops it silently.

use citadel_internal_service_test_common::pq::{spawn_agent, spawn_server, Server};
use citadel_internal_service_test_common::pq_accounts::{account, add_key};
use citadel_internal_service_test_common::pq_window::{FakeKey, Offer, Window};
use citadel_internal_service_test_common::setup_log;
use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
use citadel_internal_service_types::{SignInManagementOp, SignInManagementOutcome, SignInPolicy};
use std::error::Error;
use uuid::Uuid;

/// The agent's refusal (kernel/sign_in/recovery.rs `RECOVERY_ONLY`).
const RECOVERY_ONLY: &str = "signed in to recover the account";

fn phrase(typed: &str) -> Offer {
    Offer {
        recovery_phrase: Some(typed.to_string()),
        ..Default::default()
    }
}

#[tokio::test]
async fn the_phrase_opens_the_recovery_console_and_nothing_more() -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_server(Server::PostQuantum);
    let mut window = Window::open(spawn_agent().await?).await?;
    let user = account(
        &mut window,
        server,
        SignInPolicy::Password,
        &FakeKey::new(1),
    )
    .await?;
    let typed = user.registered.typed_phrase();

    let cid = window
        .sign_in(server, &user.username, &phrase(&typed))
        .await??;

    // A normal request: refused at once, saying why.
    let request_id = Uuid::new_v4();
    window
        .send(InternalServiceRequest::ListRegisteredPeers { request_id, cid })
        .await?;
    let peers = window.answer_of(request_id, None).await?.response;
    assert!(
        matches!(peers, InternalServiceResponse::ListRegisteredPeersFailure(ref f) if f.message.contains(RECOVERY_ONLY)),
        "a recovery session listed peers: {peers:?}"
    );
    // Management it may not make.
    let rename = SignInManagementOp::RenameCredential {
        id: 1,
        label: "x".into(),
    };
    let renamed = window.manage(cid, rename, None, None).await?;
    assert!(
        matches!(renamed, Err(ref message) if message.contains(RECOVERY_ONLY)),
        "a recovery session renamed a credential: {renamed:?}"
    );

    // What it is for: review the credentials, a new key, and a policy that uses it.
    let listed = window
        .manage(cid, SignInManagementOp::ListCredentials, None, None)
        .await??;
    assert!(
        matches!(listed, SignInManagementOutcome::Credentials { .. }),
        "{listed:?}"
    );
    let key = FakeKey::new(5);
    let added = window.manage(cid, add_key(), None, Some(&key)).await??;
    assert!(
        matches!(added, SignInManagementOutcome::Added { .. }),
        "{added:?}"
    );
    let key_only = SignInManagementOp::SetSignInPolicy {
        policy: SignInPolicy::KeyOnly,
    };
    let set = window.manage(cid, key_only, None, None).await??;
    assert_eq!(set, SignInManagementOutcome::PolicySet);
    window.disconnect(cid).await?;

    let with_key = Offer {
        key: Some(key),
        ..Default::default()
    };
    let signed_in = window.sign_in(server, &user.username, &with_key).await?;
    assert!(
        signed_in.is_ok(),
        "the recovered key did not sign in: {signed_in:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_malformed_phrase_is_refused_without_reaching_the_server() -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_server(Server::PostQuantum);
    let mut window = Window::open(spawn_agent().await?).await?;
    let user = account(
        &mut window,
        server,
        SignInPolicy::Password,
        &FakeKey::new(1),
    )
    .await?;
    let refused = window
        .sign_in(server, &user.username, &phrase("not a phrase"))
        .await?;
    let message = refused.expect_err("a malformed phrase signed in");
    assert!(
        message.starts_with("That is not a recovery phrase") && message.contains("24 words"),
        "{message}"
    );
    Ok(())
}

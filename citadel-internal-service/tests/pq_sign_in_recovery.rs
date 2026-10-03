//! A recovery code signs in once, to a session that may only add a security key, set the
//! sign-in policy, or sign out. The agent answers everything else with a clear refusal rather
//! than leaving it to the server, which drops it silently.

use citadel_internal_service_test_common::pq::{spawn_agent, spawn_server, Server};
use citadel_internal_service_test_common::pq_accounts::{account, add_key};
use citadel_internal_service_test_common::pq_window::{FakeKey, Offer, Window};
use citadel_internal_service_test_common::setup_log;
use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
use citadel_sdk::prelude::{SignInManagementOp, SignInManagementOutcome, SignInPolicy};
use std::error::Error;
use uuid::Uuid;

const RECOVERY_ONLY: &str = "signed in with a recovery code";

fn recovery(code: &str) -> Offer {
    Offer {
        recovery_code: Some(code.to_string()),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_recovery_session_is_restricted_and_its_code_signs_in_once() -> Result<(), Box<dyn Error>>
{
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
    let code = user.codes.0[3].clone();

    let cid = window.sign_in(&user.username, &recovery(&code)).await??;

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
    let list = window
        .manage(cid, SignInManagementOp::ListCredentials, None, None)
        .await?;
    assert!(
        matches!(list, Err(ref message) if message.contains(RECOVERY_ONLY)),
        "a recovery session listed credentials: {list:?}"
    );

    // What it is for: a new key, and a policy that uses it.
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

    let again = window.sign_in(&user.username, &recovery(&code)).await?;
    assert!(again.is_err(), "a recovery code signed in twice");
    let with_key = Offer {
        key: Some(key),
        ..Default::default()
    };
    let signed_in = window.sign_in(&user.username, &with_key).await?;
    assert!(
        signed_in.is_ok(),
        "the recovered key did not sign in: {signed_in:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_malformed_recovery_code_is_refused_without_reaching_the_server(
) -> Result<(), Box<dyn Error>> {
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
        .sign_in(&user.username, &recovery("not a code"))
        .await?;
    assert_eq!(refused, Err("That is not a recovery code".to_string()));
    Ok(())
}

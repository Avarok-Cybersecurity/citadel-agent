//! The agent's identity accounts end to end (identity doc §10 T4, T6, T14 rev b): one agent,
//! real in-process workspace servers that answer the `ServerIdentityChallenge`, driven through
//! the UI's request protocol.
//!
//! Controls, each named on its test:
//! - T4: build the username from the handle alone (no `@domain`) in `RegisterIdentity` → the
//!   second server's registration is refused as held.
//! - T6: remove the `account_server::held` check from `RegisterIdentity` → the re-created
//!   server accepts the name and the stored account is overwritten.
//! - T14: skip the challenge in `connect_identity` → the key-swapped server is accepted.

#[allow(dead_code)]
#[path = "identity_support/mod.rs"]
mod identity;
#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service_test_common as common;
use citadel_internal_service_types::{FailureReason, InternalServiceResponse, RegisterFailure};
use identity::*;
use parking_lot::Mutex;
use std::error::Error;
use std::sync::Arc;
use uuid::Uuid;

/// The agent's account record for `cid`, as `GetAccountInformation` reports it.
async fn account_of(
    sink: &mut Sink,
    stream: &mut Stream,
    cid: u64,
) -> Result<citadel_internal_service_types::AccountInformation, Box<dyn Error>> {
    let accounts = support::account(sink, stream, None).await?;
    Ok(accounts
        .into_iter()
        .find(|(c, _)| *c == cid)
        .ok_or(format!("account {cid} is gone"))?
        .1)
}

/// T4: `alice` on two servers from one agent: two usernames, two CIDs, both signed in and
/// verified; and the account signs in again by its CID alone.
#[tokio::test]
async fn one_identity_registers_the_same_handle_on_two_servers() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let a = spawn_challenge_server(None, Arc::new(Mutex::new([21; 32]))).await?;
    let b = spawn_challenge_server(None, Arc::new(Mutex::new([22; 32]))).await?;
    let store = support::temp_store();
    let (agent, _node) = spawn_identity_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;
    let id_fp = create_identity(&mut sink, &mut stream).await?;

    let mut cids = Vec::new();
    for server in [&a, &b] {
        let request_id = Uuid::new_v4();
        common::send(&mut sink, register_identity(request_id, server, "alice")).await?;
        let success = signed_in(&mut stream, request_id).await??;
        let pinned = success
            .identity
            .ok_or("ConnectSuccess carries no identity")?;
        assert_eq!(pinned.handle, "alice");
        assert_eq!(pinned.canonical_domain, server.addr);
        assert_eq!(pinned.server_fp, server.fp());
        assert_eq!(pinned.id_fp, id_fp);
        assert!(
            pinned.verified,
            "the first challenge proved the pinned fingerprint"
        );
        cids.push(success.cid);
    }
    assert_ne!(cids[0], cids[1], "alice@A and alice@B are two accounts");
    for (cid, server) in cids.iter().zip([&a, &b]) {
        let account = account_of(&mut sink, &mut stream, *cid).await?;
        assert_eq!(account.username, format!("alice@{}", server.addr));
        assert_eq!(account.identity.map(|i| i.verified), Some(true));
    }

    disconnect(&mut sink, &mut stream, cids[0]).await?;
    let request_id = Uuid::new_v4();
    common::send(&mut sink, connect_identity(request_id, cids[0])).await?;
    assert_eq!(signed_in(&mut stream, request_id).await??.cid, cids[0]);
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

/// T6: the server is re-created on the same address with a fresh database. Registering the
/// same handle there is refused before anything is dialled, and the account here is untouched.
#[tokio::test]
async fn a_held_name_on_a_recreated_server_is_refused() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let seed = Arc::new(Mutex::new([23; 32]));
    let first = spawn_challenge_server(None, seed.clone()).await?;
    let (front, route) = spawn_front(first.bound).await?;
    let a = spawn_challenge_server(Some(front), seed.clone()).await?;
    *route.lock() = a.bound;
    drop(first);
    let store = support::temp_store();
    let (agent, _node) = spawn_identity_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;
    create_identity(&mut sink, &mut stream).await?;
    let request_id = Uuid::new_v4();
    common::send(&mut sink, register_identity(request_id, &a, "alice")).await?;
    let cid = signed_in(&mut stream, request_id).await??.cid;
    let before = account_of(&mut sink, &mut stream, cid).await?;
    disconnect(&mut sink, &mut stream, cid).await?;

    // The same name, a server that has never seen alice.
    let recreated = spawn_challenge_server(Some(front), seed).await?;
    *route.lock() = recreated.bound;
    let request_id = Uuid::new_v4();
    common::send(
        &mut sink,
        register_identity(request_id, &recreated, "alice"),
    )
    .await?;
    let answer = support::expect(&mut stream, |r| match r {
        InternalServiceResponse::RegisterFailure(f) if f.request_id == Some(request_id) => {
            Some(Ok(f))
        }
        InternalServiceResponse::RegisterSuccess(s) if s.request_id == Some(request_id) => {
            Some(Err(s.cid))
        }
        _ => None,
    })
    .await?;
    let refused: RegisterFailure =
        answer.map_err(|cid| format!("re-registered as {cid}: overwritten"))?;
    assert_eq!(
        refused.reason_code,
        Some(FailureReason::UsernameHeld),
        "{refused:?}"
    );
    assert_eq!(
        refused.held_for_server.as_deref(),
        Some(recreated.addr.as_str())
    );

    let after = account_of(&mut sink, &mut stream, cid).await?;
    assert_eq!(after.username, before.username);
    assert_eq!(after.identity, before.identity, "the pin is untouched");
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

/// T14: the server's key is swapped after registration. The next sign-in is disconnected, the
/// window is warned with both fingerprints, and the account and its pin are as they were.
#[tokio::test]
async fn a_key_swapped_server_is_disconnected_and_nothing_is_repinned() -> Result<(), Box<dyn Error>>
{
    common::setup_log();
    let seed = Arc::new(Mutex::new([24; 32]));
    let a = spawn_challenge_server(None, seed.clone()).await?;
    let pinned_fp = a.fp();
    let store = support::temp_store();
    let (agent, _node) = spawn_identity_agent(&store).await?;
    let (mut sink, mut stream) = open(agent).await?;
    create_identity(&mut sink, &mut stream).await?;
    let request_id = Uuid::new_v4();
    common::send(&mut sink, register_identity(request_id, &a, "alice")).await?;
    let cid = signed_in(&mut stream, request_id).await??.cid;
    let before = account_of(&mut sink, &mut stream, cid).await?;
    disconnect(&mut sink, &mut stream, cid).await?;

    *seed.lock() = [25; 32];
    let offered_fp = a.fp();
    let request_id = Uuid::new_v4();
    common::send(&mut sink, connect_identity(request_id, cid)).await?;
    let warning = support::expect(&mut stream, |r| match r {
        InternalServiceResponse::ServerIdentityMismatchNotification(w) => Some(Ok(w)),
        InternalServiceResponse::ConnectSuccess(s) if s.request_id == Some(request_id) => Some(
            Err(format!("the key-swapped server was accepted as {}", s.cid)),
        ),
        _ => None,
    })
    .await??;
    assert_eq!(warning.cid, cid);
    assert_eq!(warning.domain, a.addr);
    assert_eq!(warning.pinned_fp, pinned_fp);
    assert_eq!(warning.offered_fp, offered_fp);
    let refused = signed_in(&mut stream, request_id).await?;
    let refused = refused.err().ok_or("the sign-in succeeded")?;
    assert!(refused.contains("ServerIdentityMismatch"), "{refused}");

    let session = support::session(&mut sink, &mut stream, cid).await;
    assert!(
        session.is_err(),
        "the session was disconnected: {session:?}"
    );
    let after = account_of(&mut sink, &mut stream, cid).await?;
    assert_eq!(after.identity, before.identity, "never re-pinned");
    let _ = std::fs::remove_dir_all(&store);
    Ok(())
}

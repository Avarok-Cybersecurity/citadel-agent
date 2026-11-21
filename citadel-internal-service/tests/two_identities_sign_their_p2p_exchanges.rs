//! Two agents, each holding its own identity, connect their accounts P2P through a real
//! in-process server, driven through the UI's request protocol.
//!
//! - Each end's `PeerConnectSuccess` names the other's identity fingerprint, and
//!   `GetPeerIdentity` shows both ends' fingerprints for the safety-number view.
//! - An agent that pinned another key for the peer refuses the connection, tells its window
//!   which key changed, and connects only after `AcceptPeerIdentityKey` names the new key.
//!   Control: pin the presented key in `PinBook::record_change` (auto-accept) → the window is
//!   never told which key changed and this test fails (as do two `book_tests`).
//!   Control: leave `p2p_kex_signer` unset in `ConnectIdentity` → both ends are unverified and
//!   the first test fails.
//! - A legacy account (typed password, no identity key) connects unverified, while it still
//!   checks its identity peer's signature.

#[allow(dead_code)]
#[path = "identity_support/mod.rs"]
mod identity;
#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_identity::{base64url_encode, fingerprint_grouped, Identity};
use citadel_internal_service_test_common as common;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, PeerIdentityMark,
};
use identity::p2p::*;
use identity::*;
use parking_lot::Mutex;
use std::error::Error;
use std::sync::Arc;
use uuid::Uuid;

#[tokio::test]
async fn each_end_learns_the_others_identity_fingerprint() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let server = spawn_challenge_server(None, Arc::new(Mutex::new([31; 32]))).await?;
    let (agent_a, _a) = spawn_identity_agent(&support::temp_store()).await?;
    let (agent_b, _b) = spawn_identity_agent(&support::temp_store()).await?;
    let (mut alice, alice_fp) = identity_end(agent_a, &server, "alice").await?;
    let (mut bob, bob_fp) = identity_end(agent_b, &server, "bob").await?;
    peer_register(&mut alice, &mut bob).await?;
    let [at_alice, at_bob] = connect(&mut alice, &mut bob).await?;
    assert_eq!(at_alice, verified(&grouped(&bob_fp)));
    assert_eq!(at_bob, verified(&grouped(&alice_fp)));

    let request_id = Uuid::new_v4();
    common::send(
        &mut alice.sink,
        InternalServiceRequest::GetPeerIdentity {
            request_id,
            cid: alice.cid,
            peer_cid: bob.cid,
        },
    )
    .await?;
    let info = expect(&mut alice.stream, |r| match r {
        InternalServiceResponse::PeerIdentity(info) if info.request_id == Some(request_id) => {
            Some(info)
        }
        _ => None,
    })
    .await?;
    assert_eq!(info.own_fingerprint, Some(grouped(&alice_fp)));
    assert_eq!(info.peer_fingerprint, Some(grouped(&bob_fp)));
    assert_eq!(info.pending_fingerprint, None);
    Ok(())
}

#[tokio::test]
async fn a_changed_key_is_refused_until_the_person_accepts_it() -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let server = spawn_challenge_server(None, Arc::new(Mutex::new([32; 32]))).await?;
    let cid_of = |handle: &str| {
        citadel_sdk::types::user::username_to_cid(&format!("{handle}@{}", server.addr))
    };
    let before = Identity::from_seed(&[99; 32]);
    let book = serde_json::json!([{
        "local_cid": cid_of("alice"),
        "peer_cid": cid_of("bob"),
        "key": base64url_encode(before.pk()),
        "pending": null,
    }]);
    let pins = Arc::new(PinnedBefore(Mutex::new(Some(serde_json::to_vec(&book)?))));
    let (agent_a, _a) = spawn_identity_agent_pinning(&support::temp_store(), pins).await?;
    let (agent_b, _b) = spawn_identity_agent(&support::temp_store()).await?;
    let (mut alice, _) = identity_end(agent_a, &server, "alice").await?;
    let (mut bob, bob_fp) = identity_end(agent_b, &server, "bob").await?;
    assert_eq!(alice.cid, cid_of("alice"));
    peer_register(&mut alice, &mut bob).await?;

    // The refused round: alice hears both the failure and which key changed, in either order.
    common::send(&mut alice.sink, dial(alice.cid, bob.cid)).await?;
    expect(&mut bob.stream, |r| {
        matches!(r, InternalServiceResponse::PeerConnectNotification(_)).then_some(())
    })
    .await?;
    common::send(&mut bob.sink, accept(bob.cid, alice.cid)).await?;
    let (mut refusal, mut changed) = (None, None);
    while refusal.is_none() || changed.is_none() {
        expect(&mut alice.stream, |r| match r {
            InternalServiceResponse::PeerConnectSuccess(_) => {
                panic!("connected under a changed key")
            }
            InternalServiceResponse::PeerConnectFailure(f) => {
                refusal = Some(f.message);
                Some(())
            }
            InternalServiceResponse::PeerIdentityKeyChangedNotification(n) => {
                changed = Some(n);
                Some(())
            }
            _ => None,
        })
        .await?;
    }
    let (refusal, changed) = (refusal.unwrap(), changed.unwrap());
    assert!(
        refusal.contains("identity key changed"),
        "refused as: {refusal}"
    );
    assert_eq!(
        changed.pinned_fingerprint,
        fingerprint_grouped(before.public_key())
    );
    assert_eq!(changed.presented_fingerprint, grouped(&bob_fp));

    let request_id = Uuid::new_v4();
    let wrong = fingerprint_grouped(before.public_key());
    common::send(
        &mut alice.sink,
        InternalServiceRequest::AcceptPeerIdentityKey {
            request_id,
            cid: alice.cid,
            peer_cid: bob.cid,
            fingerprint: wrong,
        },
    )
    .await?;
    expect(&mut alice.stream, |r| matches!(r, InternalServiceResponse::PeerIdentityFailure(f) if f.request_id == Some(request_id)).then_some(())).await?;

    let request_id = Uuid::new_v4();
    common::send(
        &mut alice.sink,
        InternalServiceRequest::AcceptPeerIdentityKey {
            request_id,
            cid: alice.cid,
            peer_cid: bob.cid,
            fingerprint: changed.presented_fingerprint.clone(),
        },
    )
    .await?;
    let accepted = expect(&mut alice.stream, |r| match r {
        InternalServiceResponse::PeerIdentityKeyAccepted(a) if a.request_id == Some(request_id) => {
            Some(a)
        }
        _ => None,
    })
    .await?;
    assert_eq!(accepted.fingerprint, grouped(&bob_fp));

    let [at_alice, _] = connect(&mut alice, &mut bob).await?;
    assert_eq!(at_alice, verified(&grouped(&bob_fp)));
    Ok(())
}

#[tokio::test]
async fn a_legacy_account_connects_unverified_and_still_checks_its_peer(
) -> Result<(), Box<dyn Error>> {
    common::setup_log();
    let server = spawn_challenge_server(None, Arc::new(Mutex::new([33; 32]))).await?;
    let (agent_a, _a) = spawn_identity_agent(&support::temp_store()).await?;
    let (agent_b, _b) = spawn_identity_agent(&support::temp_store()).await?;
    let (mut alice, alice_fp) = identity_end(agent_a, &server, "alice").await?;
    let (mut sink, mut stream) = open(agent_b).await?;
    let cid = support::register(&mut sink, &mut stream, &server.addr, "legacy_bob").await??;
    let mut bob = End { sink, stream, cid };
    peer_register(&mut alice, &mut bob).await?;
    let [at_alice, at_bob] = connect(&mut alice, &mut bob).await?;
    assert_eq!(
        at_alice,
        Ok(PeerIdentityMark::Unverified),
        "a legacy account signs nothing"
    );
    assert_eq!(
        at_bob,
        verified(&grouped(&alice_fp)),
        "it still pins and checks its peer"
    );
    Ok(())
}

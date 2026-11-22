//! The recovery phrase recovers the account's groups, and what was sent to them while it was
//! away. The server keeps each member's group state sealed under the account's RE-VFS key, whose
//! root is the phrase's seed, so a second device recovered with the phrase opens it, resumes the
//! group, reads what the server held for it, and its window is told the group came back from the
//! account's backup (`GroupRestoredNotification { source: Backup }`).
//!
//! The server prompts the restore as soon as the sign-in succeeds, before the agent records the
//! session, so the key also answers for a session still being dialled (revfs_seed/dialing.rs;
//! the ordering itself is covered by `revfs_seed::key_source_tests`, since on loopback the agent
//! records the session before the prompt is answered).
//!
//! Control (turns this test red): in revfs_seed/mod.rs, have `key_source` answer `None` for
//! every account (no state is kept sealed for the account, and the recovered device rejoins
//! afresh and never reads what was held for it).

use citadel_internal_service_test_common::bridge_to_test;
use citadel_internal_service_test_common::group::{
    create_and_join, group_message_arrives, peered, recv_until, send_group_message,
};
use citadel_internal_service_test_common::group_rejoin::{send_until_held_for, sign_out};
use citadel_internal_service_test_common::pq::{
    spawn_agent, spawn_server, username, Server, PASSWORD,
};
use citadel_internal_service_test_common::pq_accounts::password_offer;
use citadel_internal_service_test_common::pq_window::{Offer, Window};
use citadel_internal_service_test_common::{setup_log, PeerHandle};
use citadel_internal_service_types::{
    InternalServiceResponse, SignInManagementOp, SignInManagementOutcome,
};
use citadel_sdk::prelude::{GroupStateSource, SecBuffer};
use std::error::Error;
use std::net::SocketAddr;

const NEW_PASSWORD: &str = "a fresh start on a new laptop";

async fn signed_in(server: SocketAddr, user: &str) -> Result<(PeerHandle, String), Box<dyn Error>> {
    let mut window = Window::open(spawn_agent().await?).await?;
    let registered = window.register(server, user, PASSWORD).await??;
    let cid = window.sign_in(server, user, &password_offer()).await??;
    let (tx, rx) = bridge_to_test(window.sink, window.stream);
    Ok(((tx, rx, cid), registered.typed_phrase()))
}

#[tokio::test]
async fn a_device_recovered_with_the_phrase_reads_what_its_group_held_for_it(
) -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_server(Server::PostQuantum);
    let alice_name = username("grouprecover");
    let (mut alice, phrase) = signed_in(server, &alice_name).await?;
    let (mut bob, _) = signed_in(server, &username("groupowner")).await?;
    peered(&mut bob, &mut alice).await?;
    let group_key =
        create_and_join(&bob.0, &mut bob.1, bob.2, &alice.0, &mut alice.1, alice.2).await?;

    send_group_message(&bob.0, bob.2, group_key, b"before");
    group_message_arrives(&mut alice.1, b"before")
        .await
        .ok_or("control: no delivery to the first device")?;
    sign_out(&alice.0, &mut alice.1, alice.2).await?;

    let held =
        send_until_held_for(&bob.0, &mut bob.1, bob.2, group_key, alice.2, "while away").await;

    let mut second = Window::open(spawn_agent().await?).await?;
    let recovered = second
        .recover(server, &alice_name, &phrase, "New laptop")
        .await?;
    assert_eq!(recovered.recovered_cid, Some(alice.2));
    let console = recovered.sign_in?;
    let set = SignInManagementOp::SetPassword {
        new_password: SecBuffer::from(NEW_PASSWORD),
    };
    let set = second.manage(console, set, None, None).await??;
    assert_eq!(set, SignInManagementOutcome::PasswordSet);
    second.disconnect(console).await?;
    let offer = Offer {
        password: Some(NEW_PASSWORD.to_string()),
        ..Default::default()
    };
    let full = second.sign_in(server, &alice_name, &offer).await??;
    assert_eq!(full, alice.2, "a CID is permanent per account");

    let (_tx, mut rx) = bridge_to_test(second.sink, second.stream);
    let read = group_message_arrives(&mut rx, &held)
        .await
        .ok_or("the recovered device never read what its group held for it")?;
    assert_eq!((read.cid, read.group_key), (alice.2, group_key));
    assert_eq!(read.peer_cid, bob.2, "the signed sender");

    // After the replay, the window is told the group came back from the account's backup.
    let restored = recv_until(&mut rx, "the restore notice", |r| {
        matches!(r, InternalServiceResponse::GroupRestoredNotification(n) if n.group_key == group_key)
    })
    .await;
    let InternalServiceResponse::GroupRestoredNotification(restored) = restored else {
        unreachable!()
    };
    assert_eq!(
        (restored.cid, restored.source),
        (alice.2, GroupStateSource::Backup)
    );
    Ok(())
}

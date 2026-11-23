//! A dropped link is reconnected with the password the session was opened with. A session a
//! security key opened cannot be: the reconnect would need a touch nobody asked the user for.
//! So it gives up at once and says why, and the user signs in again. A password-only
//! post-quantum session still comes back by itself.

#[allow(dead_code)]
#[path = "reconnect_support/mod.rs"]
mod reconnect;
#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service_test_common::pq::{spawn_agent, spawn_server, Server};
use citadel_internal_service_test_common::pq::{username, PASSWORD};
use citadel_internal_service_test_common::pq_accounts::{account, add_key, password_offer};
use citadel_internal_service_test_common::pq_window::{FakeKey, Offer, Window};
use citadel_internal_service_test_common::setup_log;
use citadel_internal_service_types::{SignInManagementOp, SignInManagementOutcome, SignInPolicy};
use citadel_sdk::prelude::SecBuffer;
use reconnect::{next_link_event, Proxy};
use std::error::Error;

async fn after_a_drop(policy: SignInPolicy) -> Result<Vec<String>, Box<dyn Error>> {
    setup_log();
    let proxy = Proxy::start(spawn_server(Server::PostQuantum)).await?;
    let mut window = Window::open(spawn_agent().await?).await?;
    let key = FakeKey::new(7);
    let user = account(&mut window, proxy.addr, policy, &key).await?;
    let offer = Offer {
        key: Some(key),
        ..password_offer()
    };
    let cid = window.sign_in(proxy.addr, &user.username, &offer).await??;

    proxy.sever();
    link_events_to_the_end(&mut window, cid).await
}

/// What the window hears of `cid`'s link from a drop until it is back or gone.
async fn link_events_to_the_end(
    window: &mut Window,
    cid: u64,
) -> Result<Vec<String>, Box<dyn Error>> {
    let mut events = vec![next_link_event(&mut window.stream, cid).await?];
    while !matches!(
        events.last().map(String::as_str),
        Some("reconnected" | "disconnected")
    ) {
        events.push(next_link_event(&mut window.stream, cid).await?);
    }
    Ok(events)
}

/// A session the password alone opened, on a new account, behind a proxy that can cut it.
async fn password_session() -> Result<(Proxy, Window, u64), Box<dyn Error>> {
    setup_log();
    let proxy = Proxy::start(spawn_server(Server::PostQuantum)).await?;
    let mut window = Window::open(spawn_agent().await?).await?;
    let name = username("acct");
    let _ = window.register(proxy.addr, &name, PASSWORD).await??;
    let cid = window
        .sign_in(proxy.addr, &name, &password_offer())
        .await??;
    Ok((proxy, window, cid))
}

/// The session signed in with the password alone, and THEN its account was given a key and a
/// policy that asks for it (what the step after registration offers). The password the session
/// kept no longer signs the account in, so the reconnect must not try it.
#[tokio::test]
async fn a_session_whose_account_came_to_need_a_key_gives_up_at_once() -> Result<(), Box<dyn Error>>
{
    let (proxy, mut window, cid) = password_session().await?;
    let key = FakeKey::new(7);
    let added = window
        .manage(cid, add_key(), Some(PASSWORD), Some(&key))
        .await??;
    assert!(
        matches!(added, SignInManagementOutcome::Added { .. }),
        "{added:?}"
    );
    let policy = SignInManagementOp::SetSignInPolicy {
        policy: SignInPolicy::PasswordAndKey,
    };
    let set = window
        .manage(cid, policy, Some(PASSWORD), Some(&key))
        .await??;
    assert_eq!(set, SignInManagementOutcome::PolicySet);

    proxy.sever();
    assert_eq!(
        link_events_to_the_end(&mut window, cid).await?,
        [
            "lost(reconnecting=true)".to_string(),
            "failed(This account signs in with a security key; sign in again to reconnect)"
                .to_string(),
            "disconnected".to_string(),
        ]
    );
    Ok(())
}

/// The session changed its account's password: the reconnect signs in with the new one.
#[tokio::test]
async fn a_session_reconnects_with_the_password_it_changed_to() -> Result<(), Box<dyn Error>> {
    let (proxy, mut window, cid) = password_session().await?;
    let change = SignInManagementOp::SetPassword {
        new_password: SecBuffer::from("a different battery staple"),
    };
    let set = window.manage(cid, change, Some(PASSWORD), None).await??;
    assert_eq!(set, SignInManagementOutcome::PasswordSet);

    proxy.sever();
    assert_eq!(
        link_events_to_the_end(&mut window, cid).await?,
        [
            "lost(reconnecting=true)".to_string(),
            "reconnected".to_string()
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_key_gated_session_gives_up_at_once_instead_of_reconnecting() -> Result<(), Box<dyn Error>>
{
    let events = after_a_drop(SignInPolicy::PasswordAndKey).await?;
    assert_eq!(
        events,
        [
            "lost(reconnecting=true)".to_string(),
            "failed(This account signs in with a security key; sign in again to reconnect)"
                .to_string(),
            "disconnected".to_string(),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_password_only_post_quantum_session_still_reconnects() -> Result<(), Box<dyn Error>> {
    let events = after_a_drop(SignInPolicy::Password).await?;
    assert_eq!(
        events,
        [
            "lost(reconnecting=true)".to_string(),
            "reconnected".to_string()
        ]
    );
    Ok(())
}

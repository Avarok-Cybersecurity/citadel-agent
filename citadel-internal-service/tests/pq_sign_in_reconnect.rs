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
use citadel_internal_service_test_common::pq_accounts::{account, password_offer};
use citadel_internal_service_test_common::pq_window::{FakeKey, Offer, Window};
use citadel_internal_service_test_common::setup_log;
use citadel_sdk::prelude::SignInPolicy;
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
    let cid = window.sign_in(&user.username, &offer).await??;

    proxy.sever();
    let mut events = vec![next_link_event(&mut window.stream, cid).await?];
    while !matches!(
        events.last().map(String::as_str),
        Some("reconnected" | "disconnected")
    ) {
        events.push(next_link_event(&mut window.stream, cid).await?);
    }
    Ok(events)
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

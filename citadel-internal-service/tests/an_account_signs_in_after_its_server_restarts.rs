//! An account outlives the server process it registered on: a server that stops and starts
//! again over the same store and the same sign-in settings (what a hosted workspace does when
//! its host restarts it) still signs the account in, and the agent's reconnect brings the
//! session back by itself.
//!
//! Every other reconnect test cuts the link and leaves the server running, so none of them
//! could tell an account the store holds from one the running process remembers.
//!
//! The control is a server that comes back with another OPRF seed: it holds the account and
//! can no longer verify its password, so it answers "Authentication failed" to a sign-in that
//! is correct. The agent must stop at that answer, not repeat it for ten minutes.

#[allow(dead_code)]
#[path = "reconnect_support/mod.rs"]
mod reconnect;
#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

#[allow(dead_code)]
#[path = "server_restart_support/mod.rs"]
mod restart;

use citadel_internal_service_test_common::pq::{spawn_agent, username, PASSWORD};
use citadel_internal_service_test_common::pq_accounts::password_offer;
use citadel_internal_service_test_common::pq_window::Window;
use citadel_internal_service_test_common::setup_log;
use restart::{link_events_to_the_end, listener, stop, Door, Host};
use std::error::Error;
use std::time::Duration;

#[tokio::test]
async fn a_fresh_sign_in_works_on_the_restarted_server() -> Result<(), Box<dyn Error>> {
    setup_log();
    let host = Host::new();
    let (first, upstream) = host.start(listener()?);
    let door = Door::open(upstream).await?;
    let addr = door.addr;
    let mut window = Window::open(spawn_agent().await?).await?;
    let name = username("restart");
    let registered = window.register(addr, &name, PASSWORD).await??;

    stop(first).await;
    let (_second, upstream) = host.start(listener()?);
    door.lead_to(upstream);

    let cid = window.sign_in(addr, &name, &password_offer()).await??;
    assert_eq!(cid, registered.cid);
    Ok(())
}

#[tokio::test]
async fn a_signed_in_session_comes_back_after_its_server_restarts() -> Result<(), Box<dyn Error>> {
    setup_log();
    let host = Host::new();
    let (first, upstream) = host.start(listener()?);
    let door = Door::open(upstream).await?;
    let addr = door.addr;
    let mut window = Window::open(spawn_agent().await?).await?;
    let name = username("restart");
    let _ = window.register(addr, &name, PASSWORD).await??;
    let cid = window.sign_in(addr, &name, &password_offer()).await??;

    stop(first).await;
    let (_second, upstream) = host.start(listener()?);
    door.lead_to(upstream);

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
async fn a_server_that_refuses_the_kept_password_ends_the_reconnect_at_once(
) -> Result<(), Box<dyn Error>> {
    setup_log();
    let mut host = Host::new();
    let (first, upstream) = host.start(listener()?);
    let door = Door::open(upstream).await?;
    let addr = door.addr;
    let mut window = Window::open(spawn_agent().await?).await?;
    let name = username("restart");
    let _ = window.register(addr, &name, PASSWORD).await??;
    let cid = window.sign_in(addr, &name, &password_offer()).await??;

    stop(first).await;
    host.seed = [0xa5; 32];
    let (_second, upstream) = host.start(listener()?);
    door.lead_to(upstream);

    let started = std::time::Instant::now();
    assert_eq!(
        link_events_to_the_end(&mut window, cid).await?,
        [
            "lost(reconnecting=true)".to_string(),
            "failed(Authentication failed)".to_string(),
            "disconnected".to_string(),
        ]
    );
    // One attempt, after the first wait: nowhere near a second one's backoff.
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    Ok(())
}

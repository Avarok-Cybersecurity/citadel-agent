//! Regenerating the recovery phrase gives the account a new RE-VFS root, and every object it
//! stored is sealed again under it (kernel/revfs_seed/rekey.rs): files stored before still read
//! after, a device recovered with the new phrase reads them, and the old phrase's root opens
//! nothing stored since. A regeneration in the recovery console (which may not store) re-keys at
//! the next full sign-in, after an agent restart too: the retired root and the index persist.
//!
//! Controls (each turns a test red): in requests/sign_in_management.rs, skip `new_root` (the
//! objects stay under the old key: the first test's wait for the new key gives up); in
//! requests/connect.rs, drop the `resume` after a full sign-in (the console test's does).

#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service::kernel::revfs_seed::{AWAITS_REKEY, REKEYING};
use citadel_internal_service_test_common::pq::{spawn_agent, username, PASSWORD};
use citadel_internal_service_test_common::pq_accounts::password_offer;
use citadel_internal_service_test_common::pq_files::{
    key_id, seed_of, spawn_storing_server, stored_key_ids,
};
use citadel_internal_service_test_common::pq_window::{Offer, Window};
use citadel_internal_service_test_common::setup_log;
use citadel_internal_service_types::{SignInManagementOp, SignInManagementOutcome};
use citadel_sdk::prelude::SecBuffer;
use std::error::Error;
use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

const FILE: &str = "../resources/test.txt";
const PATHS: [&str; 2] = ["/vfs/a.txt", "/vfs/dir/b.txt"];
/// A pass re-keys two small objects in well under a second here; this only bounds a hang.
const REKEY_BOUND: Duration = Duration::from_secs(60);

async fn regenerate(
    window: &mut Window,
    cid: u64,
    step_up: Option<&str>,
) -> Result<String, Box<dyn Error>> {
    let op = SignInManagementOp::RegenerateRecoveryPhrase;
    let SignInManagementOutcome::RecoveryPhrase(words) =
        window.manage(cid, op, step_up, None).await??
    else {
        return Err("the regeneration issued no phrase".into());
    };
    Ok(words.0.join(" "))
}

/// Waits until every object the server holds is sealed under `key`.
async fn sealed_under(store: &Path, key: [u8; 8]) -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    loop {
        let stored = stored_key_ids(store);
        if !stored.is_empty() && stored.iter().all(|(_, id)| *id == key) {
            return Ok(());
        }
        if started.elapsed() > REKEY_BOUND {
            return Err(format!("not re-keyed: {stored:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn phrase(typed: &str) -> Offer {
    Offer {
        recovery_phrase: Some(typed.to_string()),
        ..Default::default()
    }
}

/// Whether the agent said "try again once the re-key has finished": the answer a window is
/// designed to wait out, not a failure.
fn rekey_under_way(refusal: &str) -> bool {
    refusal == REKEYING || refusal == AWAITS_REKEY
}

/// Each path's contents, once the re-key lets the reads through.
async fn reads_back(window: &mut Window, cid: u64, paths: &[&str]) -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    for path in paths {
        let contents = loop {
            match window.read(cid, path).await? {
                Err(refusal) if rekey_under_way(&refusal) && started.elapsed() < REKEY_BOUND => {
                    tokio::time::sleep(Duration::from_millis(200)).await
                }
                read => break read?,
            }
        };
        assert_eq!(
            contents,
            std::fs::read(FILE)?,
            "{path} reads something else"
        );
    }
    Ok(())
}

#[tokio::test]
async fn files_stored_before_a_regeneration_read_after_it_under_the_new_root(
) -> Result<(), Box<dyn Error>> {
    setup_log();
    let store = tempfile::tempdir()?;
    let server = spawn_storing_server(store.path());
    let mut window = Window::open(spawn_agent().await?).await?;
    let user = username("rekey");
    let old = window
        .register(server, &user, PASSWORD)
        .await??
        .typed_phrase();
    let cid = window.sign_in(server, &user, &password_offer()).await??;
    for path in PATHS {
        window.store(cid, Path::new(FILE), path).await??;
    }
    let old_key = key_id(&seed_of(&old), server, cid);
    sealed_under(store.path(), old_key).await?;

    let new = regenerate(&mut window, cid, Some(PASSWORD)).await?;
    let new_key = key_id(&seed_of(&new), server, cid);
    sealed_under(store.path(), new_key).await?;
    reads_back(&mut window, cid, &PATHS).await?;

    // Stored since: under the new root, which the old phrase's does not open.
    window.store(cid, Path::new(FILE), "/vfs/c.txt").await??;
    sealed_under(store.path(), new_key).await?;
    assert_ne!(old_key, new_key);
    window.disconnect(cid).await?;

    let recovered = recovered_on_another_device(server, &user, &new).await?;
    let (mut second, full) = recovered;
    reads_back(&mut second, full, &[PATHS[0], PATHS[1], "/vfs/c.txt"]).await
}

/// A second device recovers the account with `typed`, sets a password in the console and signs
/// in fully with it.
async fn recovered_on_another_device(
    server: SocketAddr,
    user: &str,
    typed: &str,
) -> Result<(Window, u64), Box<dyn Error>> {
    let mut second = Window::open(spawn_agent().await?).await?;
    let console = second
        .recover(server, user, typed, "Laptop")
        .await?
        .sign_in?;
    let set = SignInManagementOp::SetPassword {
        new_password: SecBuffer::from("after the regeneration"),
    };
    second.manage(console, set, None, None).await??;
    second.disconnect(console).await?;
    let offer = Offer {
        password: Some("after the regeneration".to_string()),
        ..Default::default()
    };
    let full = second.sign_in(server, user, &offer).await??;
    Ok((second, full))
}

#[tokio::test]
async fn a_regeneration_in_the_console_rekeys_at_the_next_full_sign_in_after_a_restart(
) -> Result<(), Box<dyn Error>> {
    setup_log();
    let store = tempfile::tempdir()?;
    let agent_store = tempfile::tempdir()?;
    let server = spawn_storing_server(store.path());
    let (agent, node) = support::spawn_agent(agent_store.path()).await?;
    let mut window = Window::open(agent).await?;
    let user = username("console");
    let old = window
        .register(server, &user, PASSWORD)
        .await??
        .typed_phrase();
    let cid = window.sign_in(server, &user, &password_offer()).await??;
    window.store(cid, Path::new(FILE), PATHS[0]).await??;
    window.disconnect(cid).await?;

    let console = window.sign_in(server, &user, &phrase(&old)).await??;
    let new = regenerate(&mut window, console, None).await?;
    window.disconnect(console).await?;
    let old_key = key_id(&seed_of(&old), server, cid);
    assert!(
        stored_key_ids(store.path())
            .iter()
            .all(|(_, id)| *id == old_key),
        "the console re-keyed: it may not store"
    );

    drop(window);
    node.abort();
    let _ = node.await;
    let (agent, _node) = support::spawn_agent(agent_store.path()).await?;
    let mut window = Window::open(agent).await?;
    let full = window.sign_in(server, &user, &password_offer()).await??;
    sealed_under(store.path(), key_id(&seed_of(&new), server, cid)).await?;
    reads_back(&mut window, full, &PATHS[..1]).await
}

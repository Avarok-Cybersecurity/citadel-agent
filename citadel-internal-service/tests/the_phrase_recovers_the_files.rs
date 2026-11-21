//! The recovery phrase recovers the account's files: its seed is the root of the account's
//! RE-VFS key, so a second device that recovers the account with the phrase reads back what the
//! first one stored. What the server holds is sealed under the phrase's key, and not under a
//! root this agent drew itself (the `seed_rec` it used to draw), which no phrase recovers.
//!
//! Controls (each turns this test red): in requests/recover_account.rs, adopt a freshly drawn
//! seed instead of the phrase's (the second device's read fails: sealed under another key); in
//! requests/register.rs, skip `store_root` (the first device's sign-in is refused: no root).

use citadel_internal_service_test_common::pq::{spawn_agent, username, PASSWORD};
use citadel_internal_service_test_common::pq_accounts::password_offer;
use citadel_internal_service_test_common::pq_files::{
    key_id, seed_of, spawn_storing_server, stored_key_ids,
};
use citadel_internal_service_test_common::pq_window::{Offer, Window};
use citadel_internal_service_test_common::setup_log;
use citadel_internal_service_types::{SignInManagementOp, SignInManagementOutcome};
use citadel_sdk::prelude::{RecoverySeed, SecBuffer};
use std::error::Error;
use std::path::Path;

const FILE: &str = "../resources/test.txt";
const NEW_PASSWORD: &str = "a fresh start on a new laptop";

#[tokio::test]
async fn a_file_stored_on_one_device_reads_back_on_another_recovered_with_the_phrase(
) -> Result<(), Box<dyn Error>> {
    setup_log();
    let store = tempfile::tempdir()?;
    let server = spawn_storing_server(store.path());
    let mut first = Window::open(spawn_agent().await?).await?;
    let user = username("files");
    let registered = first.register(server, &user, PASSWORD).await??;
    let cid = first.sign_in(server, &user, &password_offer()).await??;
    first
        .store(cid, Path::new(FILE), "/vfs/recovered.txt")
        .await??;
    first.disconnect(cid).await?;

    let typed = registered.typed_phrase();
    let phrase_key = key_id(&seed_of(&typed), server, cid);
    let drawn_root_key = key_id(RecoverySeed::generate().as_bytes(), server, cid);
    let stored = stored_key_ids(store.path());
    assert!(!stored.is_empty(), "the server holds no sealed object");
    for (path, id) in &stored {
        assert_eq!(
            *id, phrase_key,
            "{path:?} is not sealed under the phrase's key"
        );
        assert_ne!(*id, drawn_root_key);
    }

    let mut second = Window::open(spawn_agent().await?).await?;
    let recovered = second.recover(server, &user, &typed, "New laptop").await?;
    assert_eq!(recovered.recovered_cid, Some(cid));
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
    let full = second.sign_in(server, &user, &offer).await??;
    let read = second.read(full, "/vfs/recovered.txt").await?;
    assert_eq!(
        read?,
        std::fs::read(FILE)?,
        "the recovered device read something else"
    );
    Ok(())
}

//! A registration needs the recovery phrase: it recovers the account, and its seed is the root
//! of the account's files. A server that issues none (no post-quantum sign-in) has registered an
//! account nothing could recover, so the agent answers `RegisterFailure`, naming why, and stores
//! no root.
//!
//! Control: in requests/register.rs, answer `RegisterSuccess` with an empty phrase when the server
//! issued none → this test goes red.

use citadel_internal_service_test_common::pq::{spawn_agent, username, PASSWORD};
use citadel_internal_service_test_common::pq_window::Window;
use citadel_internal_service_test_common::{server_test_node_skip_cert_verification, setup_log};
use citadel_sdk::prefabs::server::empty::EmptyKernel;
use citadel_sdk::prelude::*;
use std::error::Error;

#[tokio::test]
async fn a_server_that_issues_no_phrase_fails_the_registration() -> Result<(), Box<dyn Error>> {
    setup_log();
    let (node, server) = server_test_node_skip_cert_verification(
        EmptyKernel::<StackedRatchet>::default(),
        |builder| {
            let _ = builder.with_server_misc_settings(ServerMiscSettings::default());
        },
    );
    drop(tokio::spawn(node));
    let mut window = Window::open(spawn_agent().await?).await?;
    let refused = window
        .register(server, &username("nophrase"), PASSWORD)
        .await?;
    let message = refused.expect_err("a registration without a phrase succeeded");
    assert!(
        message.contains("issued no recovery phrase"),
        "the refusal does not say why: {message}"
    );
    Ok(())
}

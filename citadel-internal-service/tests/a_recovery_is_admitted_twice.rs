//! On a server that requires an admission (Turnstile) token, a recovery is checked twice: the
//! recovery, then the phrase sign-in after it, both as `recover`, and a token is single-use.
//! After a fresh recovery the sign-in goes without one and is answered `AdmissionRequired` with
//! the challenge, under the same request, after `RecoverAccountSuccess`; the window sends
//! `RecoverAccount` again with a fresh token, and this device, now holding the account, spends it
//! on the sign-in alone.
//!
//! Control: in requests/recover_account.rs, send the held device's sign-in without the token
//! (`sign_in_admission = None`) → the second recovery is refused too and this test goes red.

use citadel_internal_service_test_common::pq::{
    spawn_agent, spawn_guarded_server, username, GOOD_TOKEN, PASSWORD,
};
use citadel_internal_service_test_common::pq_window::Window;
use citadel_internal_service_test_common::setup_log;
use citadel_internal_service_types::FailureReason;
use std::error::Error;

#[tokio::test]
async fn a_fresh_recovery_asks_for_a_second_token_for_its_sign_in() -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_guarded_server();
    let mut first = Window::open(spawn_agent().await?).await?;
    let user = username("adm-rec");
    let registered = first
        .register_admitted(server, &user, PASSWORD, Some(GOOD_TOKEN))
        .await?
        .map_err(|(message, _)| message)?;
    let typed = registered.typed_phrase();

    let mut second = Window::open(spawn_agent().await?).await?;
    let unadmitted = second.recover(server, &user, &typed, "Laptop").await?;
    assert!(
        unadmitted.refused_by_recovery && unadmitted.recovered_cid.is_none(),
        "a recovery went through without a token: {unadmitted:?}"
    );
    assert_eq!(unadmitted.reason, Some(FailureReason::AdmissionRequired));

    let recovered = second
        .recover_admitted(server, &user, &typed, "Laptop", Some(GOOD_TOKEN))
        .await?;
    assert_eq!(recovered.recovered_cid, Some(registered.cid));
    assert!(
        !recovered.refused_by_recovery && recovered.sign_in.is_err(),
        "the spent token signed in too: {recovered:?}"
    );
    assert_eq!(recovered.reason, Some(FailureReason::AdmissionRequired));
    assert!(recovered.challenged, "the window was given no challenge");

    // Held now: nothing is recovered again, and without a token the sign-in is still refused.
    let tokenless = second.recover(server, &user, &typed, "Laptop").await?;
    assert_eq!(tokenless.recovered_cid, None);
    assert_eq!(tokenless.reason, Some(FailureReason::AdmissionRequired));

    let signed_in = second
        .recover_admitted(server, &user, &typed, "Laptop", Some(GOOD_TOKEN))
        .await?;
    assert_eq!(
        signed_in.recovered_cid, None,
        "the held device recovered again"
    );
    assert_eq!(signed_in.sign_in?, registered.cid);
    Ok(())
}

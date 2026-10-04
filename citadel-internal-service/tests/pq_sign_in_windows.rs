//! A session's key challenge goes to every window attached to it, and any of them may answer:
//! the user touches the key wherever they are. A sign-in has no session yet, so its challenge
//! goes only to the window signing in (pq_sign_in_challenge.rs refuses anyone else's answer).

use citadel_internal_service_test_common::multi_window::password;
use citadel_internal_service_test_common::pq::{spawn_agent, spawn_server, Server, PASSWORD};
use citadel_internal_service_test_common::pq_accounts::{add_key, password_offer};
use citadel_internal_service_test_common::pq_window::{FakeKey, Window};
use citadel_internal_service_test_common::setup_log;
use citadel_internal_service_types::SignInManagementOutcome;
use citadel_internal_service_types::{
    ConfigCommand, InternalServiceRequest, InternalServiceResponse, SecurityKeyPurpose, StepUp,
};
use citadel_sdk::prelude::SecBuffer;
use std::error::Error;
use uuid::Uuid;

#[tokio::test]
async fn a_management_challenge_reaches_every_window_and_another_window_may_answer(
) -> Result<(), Box<dyn Error>> {
    setup_log();
    let server = spawn_server(Server::PostQuantum);
    let agent = spawn_agent().await?;
    let mut first = Window::open(agent).await?;
    let user = citadel_internal_service_test_common::pq::username("win");
    first.register(server, &user, PASSWORD).await??;
    let cid = first.sign_in(&user, &password_offer()).await??;

    let mut second = Window::open(agent).await?;
    let attach_id = Uuid::new_v4();
    second
        .send(InternalServiceRequest::ConnectionManagement {
            request_id: attach_id,
            management_command: ConfigCommand::AttachSession {
                session_cid: cid,
                proof: password(PASSWORD),
            },
        })
        .await?;
    let attached = second.next_of(attach_id).await?;
    assert!(
        matches!(attached, InternalServiceResponse::SessionAttached(_)),
        "{attached:?}"
    );

    let request_id = Uuid::new_v4();
    first
        .send(InternalServiceRequest::SignInManagement {
            request_id,
            cid,
            op: add_key(),
            step_up: StepUp {
                password: Some(SecBuffer::from(PASSWORD)),
                security_key: true,
            },
        })
        .await?;
    let InternalServiceResponse::SecurityKeyChallengeNotification(challenge) =
        second.next_of(request_id).await?
    else {
        panic!("the attached window was not asked for the new key");
    };
    assert_eq!(challenge.purpose, SecurityKeyPurpose::Enrol);
    assert_eq!(challenge.cid, cid);
    second.send(FakeKey::new(3).answer(&challenge)).await?;

    let added = first.answer_of(request_id, None).await?;
    assert_eq!(
        added.challenges.len(),
        1,
        "the asking window was not told either"
    );
    assert!(
        matches!(
            added.response,
            InternalServiceResponse::SignInManagementSuccess(ref s)
                if matches!(s.outcome, SignInManagementOutcome::Added { .. })
        ),
        "the other window's touch did not add the key: {:?}",
        added.response
    );
    Ok(())
}

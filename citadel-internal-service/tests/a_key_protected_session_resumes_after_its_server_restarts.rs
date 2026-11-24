//! A session keeps coming back from dropped links without its user for as long as its resume
//! credential lasts, and no longer.
//!
//! An account whose policy asks for a security key signs in once, with its key. Its server
//! then restarts (what a hosted workspace does when its host restarts it): the agent brings
//! the session back by itself, and no window is asked to touch a key. A session that changed
//! its own account's sign-in comes back the same way.
//!
//! When the credential's time runs out the agent says so at that moment, link up or not, and
//! marks the session's row; the drop after that ends in a sign-in, by name, and the password
//! the session signed in with is not tried in the credential's place.

#[allow(dead_code)]
#[path = "reconnect_support/mod.rs"]
mod reconnect;
#[allow(dead_code)]
#[path = "server_restart_support/mod.rs"]
mod restart;
#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service_test_common::pq::{spawn_agent, username, PASSWORD};
use citadel_internal_service_test_common::pq_accounts::{account, password_offer};
use citadel_internal_service_test_common::pq_window::{FakeKey, Offer, Window, WAIT};
use citadel_internal_service_test_common::setup_log;
use citadel_internal_service_types::{
    ConnectSuccess, FailureReason, GetSessionsResponse, InternalServiceRequest,
    InternalServiceResponse, SessionDot, SessionRowStatus, SignInManagementOp,
    SignInManagementOutcome, SignInPolicy,
};
use futures::StreamExt;
use restart::{listener, stop, Door, Host};
use std::error::Error;
use std::net::SocketAddr;
use std::time::Duration;
use uuid::Uuid;

const HOUR_S: u32 = 3_600;
const DAY: Duration = Duration::from_secs(86_400);

async fn sessions(window: &mut Window) -> Result<GetSessionsResponse, Box<dyn Error>> {
    let request_id = Uuid::new_v4();
    window
        .send(InternalServiceRequest::GetSessions { request_id })
        .await?;
    match window.next_of(request_id).await? {
        InternalServiceResponse::GetSessionsResponse(sessions) => Ok(sessions),
        other => Err(format!("not a session list: {other:?}").into()),
    }
}

/// Signs in asking to stay signed in for an hour; the answer, and how often the key was asked.
async fn sign_in_to_stay(
    window: &mut Window,
    server: SocketAddr,
    name: &str,
    key: Option<&FakeKey>,
) -> Result<(ConnectSuccess, usize), Box<dyn Error>> {
    let offer = Offer {
        key: key.cloned(),
        resume_lifetime_s: Some(HOUR_S),
        ..password_offer()
    };
    let outcome = window.connect(server, name, &offer).await?;
    match outcome.response {
        InternalServiceResponse::ConnectSuccess(success) => Ok((success, outcome.challenges.len())),
        other => Err(format!("the sign-in was refused: {other:?}").into()),
    }
}

/// Everything the window hears about `cid` from now until its link is back or gone: the link
/// events in order, the reason code of a give-up, and how many key challenges arrived.
async fn through_the_drop(
    window: &mut Window,
    cid: u64,
) -> Result<(Vec<&'static str>, Option<FailureReason>, usize), Box<dyn Error>> {
    let (mut events, mut reason, mut key_asked) = (Vec::new(), None, 0);
    loop {
        let response = tokio::time::timeout(WAIT, window.stream.next()).await?;
        match response.ok_or("the agent closed the connection")? {
            InternalServiceResponse::SecurityKeyChallengeNotification(_) => key_asked += 1,
            InternalServiceResponse::ServerConnectionLost(lost) if lost.cid == cid => {
                events.push("lost")
            }
            InternalServiceResponse::ServerReconnected(back) if back.cid == cid => {
                events.push("reconnected");
                return Ok((events, reason, key_asked));
            }
            InternalServiceResponse::ServerReconnectFailed(failed) if failed.cid == cid => {
                events.push("failed");
                reason = failed.reason_code;
            }
            InternalServiceResponse::DisconnectNotification(gone) if gone.cid == cid => {
                events.push("disconnected");
                return Ok((events, reason, key_asked));
            }
            _ => {}
        }
    }
}

/// Control: have the agent keep `Reauth::NeedsUser` for a session a key opened, or send the
/// session's own change without its credential, and the session ends instead of returning.
#[tokio::test]
async fn it_comes_back_after_its_server_restarts_and_no_key_is_asked() -> Result<(), Box<dyn Error>>
{
    setup_log();
    let mut host = Host::new();
    host.resume = Some(DAY);
    let (first, upstream) = host.start(listener()?);
    let door = Door::open(upstream).await?;
    let addr = door.addr;
    let mut window = Window::open(spawn_agent().await?).await?;
    let key = FakeKey::new(7);
    let made = account(&mut window, addr, SignInPolicy::PasswordAndKey, &key).await?;

    let (signed_in, key_asked) =
        sign_in_to_stay(&mut window, addr, &made.username, Some(&key)).await?;
    let cid = signed_in.cid;
    assert_eq!(key_asked, 1, "the full sign-in asks for the key");
    assert_eq!(signed_in.resume_max_lifetime_s, Some(DAY.as_secs() as u32));
    let until = signed_in.resume_valid_until_utc.expect("a credential");

    let listed = sessions(&mut window).await?;
    let row = listed
        .sessions
        .iter()
        .find(|s| s.cid == cid)
        .expect("listed");
    assert_eq!(
        (row.status, row.dot),
        (Some(SessionRowStatus::Live), Some(SessionDot::Green))
    );
    assert_eq!(row.resume_valid_until_utc, Some(until));

    // The session changes its own account's sign-in: it still holds what brings it back.
    let key_only = SignInManagementOp::SetSignInPolicy {
        policy: SignInPolicy::KeyOnly,
    };
    let set = window
        .manage(cid, key_only, Some(PASSWORD), Some(&key))
        .await??;
    assert_eq!(set, SignInManagementOutcome::PolicySet);

    stop(first).await;
    let (_second, upstream) = host.start(listener()?);
    door.lead_to(upstream);

    let (events, reason, key_asked) = through_the_drop(&mut window, cid).await?;
    assert_eq!(events, ["lost", "reconnected"], "{reason:?}");
    assert_eq!(key_asked, 0, "the reconnect asked for the key");
    Ok(())
}

/// Control: fall back to the password when the credential has expired, and the session
/// reconnects for ever; mark the row from the link alone, and it stays green.
#[tokio::test]
async fn when_its_time_runs_out_it_is_marked_and_its_next_drop_ends_in_a_sign_in(
) -> Result<(), Box<dyn Error>> {
    setup_log();
    let mut host = Host::new();
    // The server allows two seconds, whatever is asked.
    host.resume = Some(Duration::from_secs(2));
    let (first, upstream) = host.start(listener()?);
    let door = Door::open(upstream).await?;
    let addr = door.addr;
    let mut window = Window::open(spawn_agent().await?).await?;
    let name = username("expires");
    let _ = window.register(addr, &name, PASSWORD).await??;
    let (signed_in, _) = sign_in_to_stay(&mut window, addr, &name, None).await?;
    let cid = signed_in.cid;
    assert_eq!(signed_in.resume_max_lifetime_s, Some(2));
    let until = signed_in.resume_valid_until_utc.expect("a credential");

    // At the moment it expires, with the link up, the window is told once.
    let told = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match window.stream.next().await {
                Some(InternalServiceResponse::ResumeCredentialExpired(told)) => return Some(told),
                Some(_) => {}
                None => return None,
            }
        }
    })
    .await?
    .ok_or("the agent closed the connection")?;
    assert_eq!((told.cid, told.valid_until_utc), (cid, until));

    let listed = sessions(&mut window).await?;
    let row = listed
        .sessions
        .iter()
        .find(|s| s.cid == cid)
        .expect("still signed in");
    assert_eq!(
        (row.status, row.dot),
        (
            Some(SessionRowStatus::NeedsSignIn),
            Some(SessionDot::Yellow)
        )
    );

    stop(first).await;
    let (_second, upstream) = host.start(listener()?);
    door.lead_to(upstream);

    let (events, reason, _) = through_the_drop(&mut window, cid).await?;
    assert_eq!(events, ["lost", "failed", "disconnected"]);
    assert_eq!(reason, Some(FailureReason::ResumeCredentialExpired));

    let listed = sessions(&mut window).await?;
    assert!(listed.sessions.iter().all(|s| s.cid != cid));
    let gone = listed
        .signed_out
        .iter()
        .find(|s| s.cid == cid)
        .expect("kept");
    assert_eq!(
        gone.reason_code,
        Some(FailureReason::ResumeCredentialExpired)
    );
    assert_eq!(gone.dot, Some(SessionDot::Yellow));
    Ok(())
}

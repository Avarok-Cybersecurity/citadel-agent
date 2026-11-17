//! Raising one chat's encryption level, as the UI does it, over the real SDK.
//!
//! The UI saves the level, tells the agent (`SetAccountPreferences`), drops the link
//! (`PeerDisconnect`, which releases the peer) and asks for it back (`Interest`). With a
//! supervising agent, the supervisor is the only dialler. Two defects left the chat on
//! "Reconnecting…" (owner report 2026-11-15):
//!   - the supervisor dialled at the level the SESSION was opened with, never the chat's,
//!     so the chat was reopened at Standard -- or, where the peer dialled too, its offer was
//!     declined for being below the chat's level;
//!   - it told the window it was dialling a released peer and never that the peer was back.
//!
//! Two agents on one in-process server: `a` supervises, as the shipped agent does; `b` is
//! hosted and answers for its account.

#[allow(dead_code)]
#[path = "reconnect_support/mod.rs"]
mod reconnect;
#[allow(dead_code)]
#[path = "supervisor_support/mod.rs"]
mod supervised;
#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service::AGENT_SUPERVISOR;
use citadel_internal_service_test_common as common;
use citadel_internal_service_types::{
    AccountPreferences, ChatSecurityLevel, InternalServiceRequest, InternalServiceResponse,
    PeerSecurityMinimum, SecurityLevel, SupervisorState,
};
use std::error::Error;
use supervised::interest;
use support::expect;
use uuid::Uuid;

/// The level (`SecurityLevel::value`) the next offer from `from` names, as `to`'s window is told of it.
async fn offered_level(to: &mut support::Stream, from: u64) -> Result<u8, Box<dyn Error>> {
    expect(to, |r| match r {
        InternalServiceResponse::PeerConnectNotification(n) if n.peer_cid == from => {
            Some(n.session_security_settings.security_level.value())
        }
        _ => None,
    })
    .await
}

/// The connected pair's chat at `a` raised to Extreme as the UI raises it: tell the agent, drop
/// the link, ask for it back.
async fn connect_then_raise(pair: &mut supervised::Pair) -> Result<(), Box<dyn Error>> {
    let (a, b) = (pair.a.cid, pair.b.cid);
    common::send(&mut pair.a.sink, interest(a, b, Uuid::new_v4())).await?;
    assert_eq!(
        offered_level(&mut pair.b.stream, a).await?,
        SecurityLevel::Standard.value()
    );
    expect(&mut pair.a.stream, |r| {
        matches!(r, InternalServiceResponse::PeerConnectSuccess(s) if s.peer_cid == b).then_some(())
    })
    .await?;

    // The UI's changeChatLevel: tell the agent, drop, ask for it back.
    let told = Uuid::new_v4();
    let preferences = AccountPreferences {
        security_minimums: vec![PeerSecurityMinimum {
            peer_cid: b,
            level: ChatSecurityLevel::Extreme,
        }],
        ..AccountPreferences::UI_DEFAULTS
    };
    common::send(
        &mut pair.a.sink,
        InternalServiceRequest::SetAccountPreferences {
            request_id: told,
            cid: a,
            preferences: Box::new(preferences),
        },
    )
    .await?;
    expect(&mut pair.a.stream, |r| {
        matches!(r, InternalServiceResponse::AccountPreferencesResponse(p) if p.request_id == Some(told))
            .then_some(())
    })
    .await?;
    let dropped = Uuid::new_v4();
    common::send(
        &mut pair.a.sink,
        InternalServiceRequest::PeerDisconnect {
            request_id: dropped,
            cid: a,
            peer_cid: b,
        },
    )
    .await?;
    expect(&mut pair.a.stream, |r| {
        matches!(r, InternalServiceResponse::DisconnectNotification(d) if d.request_id == Some(dropped))
            .then_some(())
    })
    .await?;
    common::send(&mut pair.a.sink, interest(a, b, Uuid::new_v4())).await?;

    Ok(())
}

/// `a`'s window sees the link back, and any "Reconnecting…" it was told answered.
async fn settles(pair: &mut supervised::Pair) -> Result<(), Box<dyn Error>> {
    let b = pair.b.cid;
    let (mut connected, mut dialing, mut healed) = (false, false, false);
    expect(&mut pair.a.stream, |r| {
        match r {
            InternalServiceResponse::PeerConnectSuccess(s) if s.peer_cid == b => connected = true,
            InternalServiceResponse::SupervisorNotification(n) if n.peer_cid == Some(b) => {
                match n.state {
                    SupervisorState::Healing => dialing = true,
                    SupervisorState::Healed => healed = true,
                    SupervisorState::Degraded => {}
                }
            }
            _ => {}
        }
        (connected && (!dialing || healed)).then_some(())
    })
    .await
    .map_err(|e| {
        format!("connected {connected}, told dialing {dialing}, told healed {healed}: {e}").into()
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_raised_chat_is_redialled_at_its_level_and_reported_back() -> Result<(), Box<dyn Error>> {
    let mut pair = supervised::pair(Some(AGENT_SUPERVISOR), None).await?;
    let a = pair.a.cid;
    connect_then_raise(&mut pair).await?;
    assert_eq!(
        offered_level(&mut pair.b.stream, a).await?,
        SecurityLevel::Extreme.value(),
        "the supervisor redials at the chat's level, not the session's"
    );
    settles(&mut pair).await
}

/// The peer did not change its level and its agent supervises too: it redials at Standard
/// and is declined. The pair still settles at the raised level -- the peer does not have to
/// agree -- and the peer's own "Reconnecting…" is answered.
#[tokio::test(flavor = "multi_thread")]
async fn only_one_side_raising_it_still_settles_both() -> Result<(), Box<dyn Error>> {
    let mut pair = supervised::pair(Some(AGENT_SUPERVISOR), Some(AGENT_SUPERVISOR)).await?;
    let a = pair.a.cid;
    connect_then_raise(&mut pair).await?;
    let (mut offered, mut healed) = (None, false);
    expect(&mut pair.b.stream, |r| {
        match r {
            InternalServiceResponse::PeerConnectNotification(n) if n.peer_cid == a => {
                offered.get_or_insert(n.session_security_settings.security_level.value());
            }
            InternalServiceResponse::SupervisorNotification(n)
                if n.peer_cid == Some(a) && n.state == SupervisorState::Healed =>
            {
                healed = true
            }
            _ => {}
        }
        (offered.is_some() && healed).then_some(())
    })
    .await
    .map_err(|e| format!("b was offered {offered:?}, told healed {healed}: {e}"))?;
    assert_eq!(offered, Some(SecurityLevel::Extreme.value()));
    settles(&mut pair).await
}

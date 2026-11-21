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

/// The level (`SecurityLevel::value`) of the next link to `peer` that `window` is told is up:
/// the level the two ends agreed, whichever of them dialled.
async fn linked_level(window: &mut support::Stream, peer: u64) -> Result<u8, Box<dyn Error>> {
    expect(window, |r| match r {
        InternalServiceResponse::PeerConnectSuccess(s) if s.peer_cid == peer => {
            Some(s.session_security_settings.security_level.value())
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
    assert_eq!(
        linked_level(&mut pair.a.stream, b).await?,
        SecurityLevel::Standard.value()
    );
    assert_eq!(
        linked_level(&mut pair.b.stream, a).await?,
        SecurityLevel::Standard.value()
    );

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

/// `a`'s window sees the link back, and any "Reconnecting…" it was told answered; the level the
/// link came back at.
async fn settles(pair: &mut supervised::Pair) -> Result<u8, Box<dyn Error>> {
    let b = pair.b.cid;
    let (mut connected, mut dialing, mut healed) = (None, false, false);
    expect(&mut pair.a.stream, |r| {
        match r {
            InternalServiceResponse::PeerConnectSuccess(s) if s.peer_cid == b => {
                connected.get_or_insert(s.session_security_settings.security_level.value());
            }
            InternalServiceResponse::SupervisorNotification(n) if n.peer_cid == Some(b) => {
                match n.state {
                    SupervisorState::Healing => dialing = true,
                    SupervisorState::Healed => healed = true,
                    SupervisorState::Degraded => {}
                }
            }
            _ => {}
        }
        connected.filter(|_| !dialing || healed)
    })
    .await
    .map_err(|e| {
        format!("connected at {connected:?}, told dialing {dialing}, told healed {healed}: {e}")
            .into()
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
    assert_eq!(settles(&mut pair).await?, SecurityLevel::Extreme.value());
    Ok(())
}

/// The peer did not change its level and its agent supervises too: it redials at Standard.
/// The pair still settles at the raised level -- the peer does not have to agree -- and the
/// peer's own "Reconnecting…" is answered.
///
/// Both ends redial, so which link comes up depends on the order the two dials reach the
/// server, and both orders are correct:
///   - the peer's Standard dial is answered first, and declined; `a`'s Extreme dial then
///     reaches the peer as an offer, which it accepts;
///   - `a`'s dial arrives while the peer's is still pending: the server merges the crossed
///     dials into one link at the stronger offer (R-DIAL), and the peer is never offered
///     anything -- its own dial succeeds, at Extreme.
///
/// So what is asserted is the level each end's link came up at, not an offer: asserting the
/// peer was offered Extreme failed whenever the dials crossed (about one run in seven).
#[tokio::test(flavor = "multi_thread")]
async fn only_one_side_raising_it_still_settles_both() -> Result<(), Box<dyn Error>> {
    let mut pair = supervised::pair(Some(AGENT_SUPERVISOR), Some(AGENT_SUPERVISOR)).await?;
    let a = pair.a.cid;
    connect_then_raise(&mut pair).await?;
    let (mut linked, mut healed) = (None, false);
    expect(&mut pair.b.stream, |r| {
        match r {
            InternalServiceResponse::PeerConnectSuccess(s) if s.peer_cid == a => {
                linked.get_or_insert(s.session_security_settings.security_level.value());
            }
            InternalServiceResponse::SupervisorNotification(n)
                if n.peer_cid == Some(a) && n.state == SupervisorState::Healed =>
            {
                healed = true
            }
            _ => {}
        }
        (linked.is_some() && healed).then_some(())
    })
    .await
    .map_err(|e| format!("b linked at {linked:?}, told healed {healed}: {e}"))?;
    assert_eq!(linked, Some(SecurityLevel::Extreme.value()));
    assert_eq!(settles(&mut pair).await?, SecurityLevel::Extreme.value());
    Ok(())
}

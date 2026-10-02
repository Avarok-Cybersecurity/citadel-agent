//! The agent's answer to an offer, rule by rule, against the UI's
//! (`incoming-connect.ts`, `pause-rules.ts`, `security-level-rank.ts`).

use super::decide::{decide, AgentAnswer, OfferFacts, PauseRecord, PAUSED_MARKER};
use citadel_internal_service_types::SecurityLevel;

fn facts() -> OfferFacts {
    OfferFacts {
        registered: Some(true),
        pause: PauseRecord::Absent,
        offered: SecurityLevel::Standard,
        minimum: Some(SecurityLevel::Standard),
    }
}

#[test]
fn a_registered_unpaused_peer_at_the_chats_level_is_accepted() {
    assert_eq!(decide(facts()), AgentAnswer::Accept);
}

#[test]
fn a_paused_contact_is_declined() {
    let paused = OfferFacts {
        pause: PauseRecord::Paused,
        ..facts()
    };
    assert_eq!(decide(paused), AgentAnswer::Decline);
}

#[test]
fn an_unreadable_pause_record_gets_no_answer_from_anyone() {
    let unknown = OfferFacts {
        pause: PauseRecord::Unreadable,
        ..facts()
    };
    assert_eq!(decide(unknown), AgentAnswer::NoAnswer);
    assert!(
        AgentAnswer::NoAnswer.agent_has_it(),
        "a window would answer it"
    );
}

#[test]
fn an_offer_below_the_chats_level_is_declined_and_at_or_above_accepted() {
    let high = |offered| OfferFacts {
        offered,
        minimum: Some(SecurityLevel::High),
        ..facts()
    };
    assert_eq!(
        decide(high(SecurityLevel::Reinforced)),
        AgentAnswer::Decline
    );
    assert_eq!(decide(high(SecurityLevel::High)), AgentAnswer::Accept);
    assert_eq!(decide(high(SecurityLevel::Extreme)), AgentAnswer::Accept);
}

#[test]
fn unreadable_preferences_get_no_answer() {
    let unknown = OfferFacts {
        minimum: None,
        ..facts()
    };
    assert_eq!(decide(unknown), AgentAnswer::NoAnswer);
}

/// A pause outranks the level, as in the UI: a paused contact is declined
/// even when the preferences cannot be read.
#[test]
fn the_pause_is_consulted_before_the_level() {
    let both = OfferFacts {
        pause: PauseRecord::Paused,
        minimum: None,
        ..facts()
    };
    assert_eq!(decide(both), AgentAnswer::Decline);
}

#[test]
fn a_peer_not_known_registered_is_left_to_windows() {
    for registered in [Some(false), None] {
        let stranger = OfferFacts {
            registered,
            ..facts()
        };
        assert_eq!(decide(stranger), AgentAnswer::LeaveToWindows);
        assert!(!AgentAnswer::LeaveToWindows.agent_has_it());
    }
}

#[test]
fn the_pause_record_reads_as_the_ui_reads_it() {
    assert_eq!(PauseRecord::from_read(&Ok(None)), PauseRecord::Absent);
    assert_eq!(
        PauseRecord::from_read(&Ok(Some(PAUSED_MARKER.to_vec()))),
        PauseRecord::Paused
    );
    assert_eq!(
        PauseRecord::from_read(&Ok(Some(b"other".to_vec()))),
        PauseRecord::Unreadable
    );
    assert_eq!(
        PauseRecord::from_read(&Err("disk".into())),
        PauseRecord::Unreadable
    );
}

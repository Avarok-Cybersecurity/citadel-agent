//! What a PeerConnect answer means to the supervisor.

use super::dialer::{chat_dial_settings, outcome_of, ALREADY_EXCHANGING_KEYS};
use crate::kernel::requests::peer::dial::ALREADY_CONNECTED;
use crate::kernel::supervisor::types::DialOutcome;
use citadel_internal_service_types::{
    InternalServiceResponse, PeerConnectFailure, SecurityLevel, SessionSecuritySettings,
};

fn refused(message: String) -> InternalServiceResponse {
    InternalServiceResponse::PeerConnectFailure(PeerConnectFailure {
        cid: 1,
        message,
        request_id: None,
    })
}

#[test]
fn a_crossed_dial_is_in_progress_as_the_sdk_words_it() {
    // As `connect_to_peer` renders the protocol's SignalError (code 333's form).
    let message = format!("P2P signal routing failed: {ALREADY_EXCHANGING_KEYS} 22");
    assert_eq!(outcome_of(&refused(message)), DialOutcome::InProgress);
}

#[test]
fn an_existing_connection_is_connected() {
    let message = format!("{ALREADY_CONNECTED} 22");
    assert_eq!(outcome_of(&refused(message)), DialOutcome::Connected);
}

#[test]
fn any_other_refusal_is_a_failure() {
    let message = "P2P signal routing failed: peer 22 is offline".to_string();
    assert_eq!(outcome_of(&refused(message)), DialOutcome::Failed);
}

fn opened_at(security_level: SecurityLevel) -> SessionSecuritySettings {
    SessionSecuritySettings {
        security_level,
        ..citadel_sdk::prelude::default_session_security_settings()
    }
}

#[test]
fn a_chat_raised_above_the_session_is_dialled_at_its_own_level() {
    let opened = opened_at(SecurityLevel::Standard);
    let dialled = chat_dial_settings(opened, SecurityLevel::Extreme);
    assert_eq!(
        dialled.security_level.value(),
        SecurityLevel::Extreme.value()
    );
    assert_eq!(
        dialled.secrecy_mode, opened.secrecy_mode,
        "only the level changes"
    );
}

#[test]
fn a_chat_below_the_session_never_lowers_it() {
    let dialled = chat_dial_settings(opened_at(SecurityLevel::High), SecurityLevel::Standard);
    assert_eq!(dialled.security_level.value(), SecurityLevel::High.value());
}

//! The agent's `citadel.session` wire carries the guest ABI's `SessionStatus` itself, not a copy,
//! and takes its limits from the ABI's: one definition, so the page relays a state unchanged.

use citadel_app_sdk_types::calls::session;
use citadel_internal_service_types::{
    AppSessionState, APP_SESSION_MAX_BODY_BYTES, APP_SESSION_MAX_SERVERS,
};

fn guest(status: citadel_app_sdk_types::calls::SessionStatus) -> Vec<u8> {
    citadel_app_sdk_types::encode(&status)
}

#[test]
fn an_agent_state_is_the_guest_abis_status_on_both_wires() {
    let state = AppSessionState {
        cid: 1,
        request_id: None,
        install_id: "i".into(),
        session: 2,
        server_cid: 3,
        state: citadel_internal_service_types::SessionStatus::Closed {
            reason: "gone".into(),
        },
    };
    let json = serde_json::to_value(&state).unwrap();
    assert_eq!(
        json["state"],
        serde_json::json!({"closed": {"reason": "gone"}})
    );
    assert_eq!(guest(state.state), b"\xa1\x66closed\xa1\x66reason\x64gone");
}

#[test]
fn the_agents_hard_limits_are_the_abis_ceilings() {
    assert_eq!(
        APP_SESSION_MAX_BODY_BYTES,
        session::SESSION_MAX_MSG_BYTES_CEILING as usize
    );
    assert_eq!(APP_SESSION_MAX_SERVERS, session::MAX_SESSION_SERVERS);
    assert_eq!(
        (APP_SESSION_MAX_BODY_BYTES, APP_SESSION_MAX_SERVERS),
        (1_048_576, 2)
    );
}

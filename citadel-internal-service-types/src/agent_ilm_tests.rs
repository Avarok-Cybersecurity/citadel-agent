//! Wire compatibility of the phase 2b additions: older peers on either side of
//! the browser/agent socket must keep working.
use crate::*;

fn request_index(request: &InternalServiceRequest) -> u32 {
    let bytes = bincode2::serialize(request).unwrap();
    u32::from_le_bytes(bytes[..4].try_into().unwrap())
}

fn response_index(response: &InternalServiceResponse) -> u32 {
    let bytes = bincode2::serialize(response).unwrap();
    u32::from_le_bytes(bytes[..4].try_into().unwrap())
}

fn enable() -> InternalServiceRequest {
    InternalServiceRequest::EnableAgentIlm {
        request_id: Uuid::from_u128(1),
        cid: 42,
    }
}

fn send_reliable() -> InternalServiceRequest {
    InternalServiceRequest::SendReliable {
        request_id: Uuid::from_u128(2),
        cid: 42,
        peer_cid: 43,
        message: b"hello".to_vec(),
        security_level: SecurityLevel::Standard,
    }
}

/// What an agent from before this field sends, byte for byte in shape.
const OLD_AGENT_SESSIONS: &str =
    r#"{"GetSessionsResponse":{"cid":0,"sessions":[],"request_id":null}}"#;

#[test]
fn a_sessions_response_from_an_older_agent_still_parses() {
    let parsed: InternalServiceResponse = serde_json::from_str(OLD_AGENT_SESSIONS).unwrap();
    let InternalServiceResponse::GetSessionsResponse(response) = parsed else {
        panic!("wrong variant");
    };
    assert_eq!(response.agent_ilm, None);
}

#[test]
fn the_offer_round_trips_and_an_older_reader_ignores_it() {
    let response = InternalServiceResponse::GetSessionsResponse(GetSessionsResponse {
        cid: 0,
        sessions: vec![],
        request_id: None,
        agent_ilm: Some(AgentIlmOffer { hosted: vec![7, 9] }),
    });
    let json = serde_json::to_string(&response).unwrap();
    let back: InternalServiceResponse = serde_json::from_str(&json).unwrap();
    let InternalServiceResponse::GetSessionsResponse(back) = back else {
        panic!("wrong variant");
    };
    assert_eq!(back.agent_ilm, Some(AgentIlmOffer { hosted: vec![7, 9] }));

    // An older reader: the same struct without the field. serde ignores the
    // unknown key, which is what the browser's JSON parse does too.
    #[derive(serde::Deserialize)]
    struct Old {
        #[allow(dead_code)]
        sessions: Vec<SessionInformation>,
    }
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let _old: Old = serde_json::from_value(value["GetSessionsResponse"].clone()).unwrap();
}

#[test]
fn the_new_requests_round_trip_over_both_encodings() {
    for request in [enable(), send_reliable()] {
        let json = serde_json::to_string(&request).unwrap();
        let from_json: InternalServiceRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(format!("{from_json:?}"), format!("{request:?}"));

        let bytes = bincode2::serialize(&request).unwrap();
        let from_bincode: InternalServiceRequest = bincode2::deserialize(&bytes).unwrap();
        assert_eq!(format!("{from_bincode:?}"), format!("{request:?}"));
        assert_eq!(from_bincode.session_cid(), Some(42), "must be gated");
    }
}

#[test]
fn the_new_responses_round_trip() {
    let responses = [
        InternalServiceResponse::EnableAgentIlmSuccess(EnableAgentIlmSuccess {
            cid: 1,
            already_hosted: true,
            request_id: Some(Uuid::from_u128(3)),
        }),
        InternalServiceResponse::EnableAgentIlmFailure(EnableAgentIlmFailure {
            cid: 1,
            message: "no".into(),
            request_id: Some(Uuid::from_u128(4)),
        }),
        InternalServiceResponse::SendReliableSuccess(SendReliableSuccess {
            cid: 1,
            peer_cid: 2,
            request_id: Some(Uuid::from_u128(5)),
        }),
        InternalServiceResponse::SendReliableFailure(SendReliableFailure {
            cid: 1,
            peer_cid: 2,
            message: "no".into(),
            request_id: Some(Uuid::from_u128(6)),
        }),
    ];
    for response in responses {
        let json = serde_json::to_string(&response).unwrap();
        let back: InternalServiceResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(format!("{back:?}"), format!("{response:?}"));
        assert_eq!(back.request_id(), response.request_id());
    }
}

/// Appended, never inserted: a variant's bincode index is its position, and a
/// peer built before this change must decode every existing variant as before.
#[test]
fn the_new_variants_are_appended_after_every_existing_one() {
    let batched = request_index(&InternalServiceRequest::Batched {
        request_id: Uuid::nil(),
        commands: vec![],
    });
    assert_eq!(request_index(&enable()), batched + 1);
    assert_eq!(request_index(&send_reliable()), batched + 2);

    let last_old = response_index(&InternalServiceResponse::BatchedResponse(
        BatchedResponseData {
            cid: 0,
            request_id: None,
            results: vec![],
        },
    ));
    let first_new = response_index(&InternalServiceResponse::EnableAgentIlmSuccess(
        EnableAgentIlmSuccess {
            cid: 0,
            already_hosted: false,
            request_id: None,
        },
    ));
    assert_eq!(first_new, last_old + 1);
}

/// The plaintext body stays out of logs, as it does for `Message`.
#[test]
fn a_reliable_send_does_not_print_its_body() {
    let debug = format!("{:?}", send_reliable());
    assert!(!debug.contains("104, 101"), "body bytes leaked: {debug}");
    assert!(debug.contains("redacted"));
}

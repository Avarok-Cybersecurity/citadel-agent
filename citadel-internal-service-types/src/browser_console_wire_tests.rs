//! A window forwards console lines only to an agent whose greeting offers it, so an older
//! agent -- whose greeting never mentions it -- must read as "no", and the request must keep
//! its JSON shape (the agent's WebSocket speaks JSON).

use crate::{
    BrowserConsoleEntry, BrowserConsoleSource, InternalServicePayload, InternalServiceRequest,
    InternalServiceResponse, ServiceConnectionAccepted,
};
use uuid::Uuid;

fn greeting(browser_console: bool) -> ServiceConnectionAccepted {
    let InternalServiceResponse::ServiceConnectionAccepted(greeting) =
        ServiceConnectionAccepted::greeting(Uuid::nil(), false, false, browser_console)
    else {
        panic!("not a greeting")
    };
    greeting
}

#[test]
fn the_greeting_offers_console_capture_only_when_told() {
    assert!(greeting(true).browser_console);
    assert!(!greeting(false).browser_console);
}

#[test]
fn an_older_agent_s_greeting_does_not_offer_it() {
    let older = r#"{"cid":0,"request_id":null,"agent_ilm":true,"stages_uploads":true}"#;
    let greeting: ServiceConnectionAccepted = serde_json::from_str(older).unwrap();
    assert!(!greeting.browser_console);
}

#[test]
fn the_request_is_gated_by_its_session() {
    let request = InternalServiceRequest::BrowserConsole {
        request_id: Uuid::nil(),
        cid: 42,
        entries: vec![],
    };
    assert_eq!(request.session_cid(), Some(42));
}

#[test]
fn the_request_reads_as_a_window_writes_it() {
    let json = r#"{"Request":{"BrowserConsole":{"request_id":"00000000-0000-0000-0000-000000000000","cid":7,"entries":[{"source":"UnhandledRejection","message":"boom","stack":null,"repeats":3}]}}}"#;
    let payload: InternalServicePayload = serde_json::from_str(json).unwrap();
    let InternalServicePayload::Request(InternalServiceRequest::BrowserConsole {
        cid,
        entries,
        ..
    }) = payload
    else {
        panic!("not a BrowserConsole request")
    };
    assert_eq!(cid, 7);
    assert_eq!(
        entries,
        [BrowserConsoleEntry {
            source: BrowserConsoleSource::UnhandledRejection,
            message: "boom".into(),
            stack: None,
            repeats: 3,
        }]
    );
}

/// What an agent that predates a request sees: a frame it cannot parse, refused as one frame.
/// The WebSocket stream reports it as an error and reads on (see the connector's test).
#[test]
fn a_request_this_agent_does_not_know_does_not_parse() {
    let json =
        r#"{"Request":{"SomeLaterRequest":{"request_id":"00000000-0000-0000-0000-000000000000"}}}"#;
    assert!(serde_json::from_str::<InternalServicePayload>(json).is_err());
}

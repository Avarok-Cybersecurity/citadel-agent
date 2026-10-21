//! The greeting tells a browser whether it may stage uploads, and an older agent's
//! greeting -- which never mentions it -- reads as "no", so the browser falls back to
//! inline `ByteContents` instead of sending requests that agent cannot parse.

use crate::{InternalServiceResponse, ServiceConnectionAccepted};
use uuid::Uuid;

#[test]
fn this_agent_s_greeting_offers_staged_uploads() {
    let InternalServiceResponse::ServiceConnectionAccepted(greeting) =
        ServiceConnectionAccepted::greeting(Uuid::nil(), false, false, false)
    else {
        panic!("not a greeting")
    };
    assert!(greeting.stages_uploads);
}

#[test]
fn an_older_agent_s_greeting_does_not() {
    let older = r#"{"cid":0,"request_id":null,"agent_ilm":true,"supervises_p2p":true}"#;
    let greeting: ServiceConnectionAccepted = serde_json::from_str(older).unwrap();
    assert!(!greeting.stages_uploads);
}

fn greeting(native_picker: bool) -> ServiceConnectionAccepted {
    let InternalServiceResponse::ServiceConnectionAccepted(greeting) =
        ServiceConnectionAccepted::greeting(Uuid::nil(), false, native_picker, false)
    else {
        panic!("not a greeting")
    };
    greeting
}

#[test]
fn the_greeting_says_whether_a_native_picker_exists() {
    assert!(greeting(true).native_picker);
    // Negative control: a build without the dialogs must not claim them.
    assert!(!greeting(false).native_picker);
}

#[test]
fn an_older_agent_s_greeting_has_no_native_picker() {
    let older = r#"{"cid":0,"request_id":null,"agent_ilm":true}"#;
    let greeting: ServiceConnectionAccepted = serde_json::from_str(older).unwrap();
    assert!(!greeting.native_picker);
}

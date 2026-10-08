//! The greeting tells a browser whether it may stage uploads, and an older agent's
//! greeting -- which never mentions it -- reads as "no", so the browser falls back to
//! inline `ByteContents` instead of sending requests that agent cannot parse.

use crate::{InternalServiceResponse, ServiceConnectionAccepted};
use uuid::Uuid;

#[test]
fn this_agent_s_greeting_offers_staged_uploads() {
    let InternalServiceResponse::ServiceConnectionAccepted(greeting) =
        ServiceConnectionAccepted::greeting(Uuid::nil(), false)
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

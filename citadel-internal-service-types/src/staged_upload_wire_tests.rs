//! The greeting tells a browser whether it may stage uploads and whether a native file
//! picker exists.

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

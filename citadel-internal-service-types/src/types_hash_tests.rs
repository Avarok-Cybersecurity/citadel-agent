//! The greeting carries this build's types hash, so a page can compare it.
use super::*;

#[test]
fn the_hash_is_a_sha3_256_digest() {
    assert_eq!(TYPES_HASH.len(), 64);
    assert!(TYPES_HASH.bytes().all(|b| b.is_ascii_hexdigit()));
}

#[test]
fn the_greeting_says_this_builds_types_hash() {
    let InternalServiceResponse::ServiceConnectionAccepted(greeting) =
        ServiceConnectionAccepted::greeting(Uuid::nil(), false, false, false)
    else {
        panic!("the greeting is ServiceConnectionAccepted");
    };
    assert_eq!(greeting.types_hash.as_deref(), Some(TYPES_HASH));
}

/// An agent from before the field greets without it: absent, not an error.
#[test]
fn an_older_agents_greeting_has_no_hash() {
    let old = r#"{"cid":0,"request_id":null,"agent_ilm":true}"#;
    let greeting: ServiceConnectionAccepted = serde_json::from_str(old).unwrap();
    assert_eq!(greeting.types_hash, None);
}

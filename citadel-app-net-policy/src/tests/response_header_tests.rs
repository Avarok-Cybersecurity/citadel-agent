//! What an upstream's response headers may show an app.

use super::*;

#[test]
fn credentials_and_framing_are_never_shown_in_any_case() {
    for name in [
        "set-cookie",
        "Set-Cookie",
        "SET-COOKIE2",
        "www-authenticate",
        "Proxy-Authenticate",
        "content-length",
        "Transfer-Encoding",
        "connection",
        "keep-alive",
        "upgrade",
        "te",
        "trailer",
        "proxy-connection",
    ] {
        assert!(!response_header_ok(name), "{name} was shown");
    }
}

#[test]
fn ordinary_response_headers_are_shown() {
    // The control: the deny-list is not a blanket refusal.
    for name in [
        "content-type",
        "Content-Type",
        "etag",
        "cache-control",
        "x-request-id",
    ] {
        assert!(response_header_ok(name), "{name} was dropped");
    }
}

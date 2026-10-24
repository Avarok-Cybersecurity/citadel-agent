//! The request headers an app may set.
//!
//! An allow-list, so framing and routing headers (`Host`, `Content-Length`,
//! `Transfer-Encoding`, `Connection`, `Upgrade`, `Forwarded`, `X-Forwarded-*`, `Proxy-*`,
//! `Origin`, `Sec-*`) can never be set by an app: the runtime writes framing itself, from the
//! checked target. `Authorization` and `Cookie` are allowed because the app sets its own; the
//! runtime never adds either.
//!
//! The response headers an app is shown are a deny-list instead ([`response_header_ok`]): an
//! upstream answers with whatever it likes, and what must never reach an app is what would
//! act on the member's behalf (cookies, authentication challenges) or describe framing the
//! runtime has already undone.

use crate::{MAX_HEADERS, MAX_HEADER_BYTES};

/// Lowercase names an app may send, besides `x-` names (other than `x-forwarded-*` and
/// `x-real-ip`).
pub const ALLOWED_HEADERS: [&str; 15] = [
    "accept",
    "accept-language",
    "authorization",
    "cache-control",
    "content-language",
    "content-type",
    "cookie",
    "if-match",
    "if-modified-since",
    "if-none-match",
    "if-range",
    "if-unmodified-since",
    "pragma",
    "range",
    "user-agent",
];

fn name_allowed(name: &str) -> bool {
    let token = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !token {
        return false;
    }
    if ALLOWED_HEADERS.contains(&name) {
        return true;
    }
    name.starts_with("x-")
        && name.len() > 2
        && !name.starts_with("x-forwarded-")
        && name != "x-real-ip"
}

fn value_allowed(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b == b'\t' || (0x20..=0x7e).contains(&b))
}

/// Whether every header is allowed: at most `MAX_HEADERS`, names lowercase and allow-listed,
/// values printable ASCII (no CR, LF or NUL), each name and value within `MAX_HEADER_BYTES`.
pub(crate) fn headers_allowed(headers: &[(String, String)]) -> bool {
    headers.len() <= MAX_HEADERS
        && headers.iter().all(|(name, value)| {
            name.len() + value.len() <= MAX_HEADER_BYTES
                && name_allowed(name)
                && value_allowed(value)
        })
}

/// Lowercase response header names an app is never shown. `set-cookie` and `set-cookie2`
/// would hand the app a credential the upstream meant for a browser; `www-authenticate` and
/// `proxy-authenticate` invite credentials; the rest are hop-by-hop framing, which the
/// runtime has already consumed, so a value here would describe bytes the app never sees.
pub const DENIED_RESPONSE_HEADERS: [&str; 12] = [
    "set-cookie",
    "set-cookie2",
    "www-authenticate",
    "proxy-authenticate",
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "upgrade",
    "te",
    "trailer",
    "proxy-connection",
];

/// Whether an upstream response header may be shown to the app. `name` in any case.
pub fn response_header_ok(name: &str) -> bool {
    !DENIED_RESPONSE_HEADERS
        .iter()
        .any(|denied| denied.eq_ignore_ascii_case(name))
}

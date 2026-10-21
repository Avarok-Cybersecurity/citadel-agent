//! The request headers an app may set.
//!
//! An allow-list, so framing and routing headers (`Host`, `Content-Length`,
//! `Transfer-Encoding`, `Connection`, `Upgrade`, `Forwarded`, `X-Forwarded-*`, `Proxy-*`,
//! `Origin`, `Sec-*`) can never be set by an app: the runtime writes framing itself, from the
//! checked target. `Authorization` and `Cookie` are allowed because the app sets its own; the
//! runtime never adds either.

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

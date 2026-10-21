//! A Citadel app's host egress: `InternalServiceRequest::AppFetch` and what answers it.
//!
//! An app never reaches the network from the page. Its host page asks the member's local agent,
//! which decides the request with the shared SSRF policy (`citadel-app-net-policy`), resolves
//! the name, decides every address, and connects only to the address it checked. The agent opens
//! no listener for this: the request is outbound only.
//!
//! The grant travels with each request because the agent holds no install records; the page
//! copies it from the install the workspace server approved. The agent trusts it for nothing
//! the policy does not also check: hosts are explicit names, and every hard-denied target
//! (the agent itself, the page's own origin, cloud metadata, non-unicast) is refused whatever
//! the grant says.

use crate::plaintext_debug_fmt;
use custom_debug::Debug;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// What the install was granted: `net.host` with these hosts, and `net.host.private`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
#[serde(deny_unknown_fields)]
pub struct AppNetGrant {
    /// `api.acme.com` or `api.acme.com:8080`; no scheme, path or wildcard; at most 8.
    pub hosts: Vec<String>,
    /// `net.host.private`: http, and private, loopback and link-local addresses.
    pub private: bool,
}

/// One request header: a lowercase name and a printable-ASCII value.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
#[serde(deny_unknown_fields)]
pub struct AppHttpHeader {
    pub name: String,
    #[debug(with = header_value_debug_fmt)]
    pub value: String,
}

/// An app sets its own `authorization` and `cookie`: a value is never printed.
fn header_value_debug_fmt<T: AsRef<str>>(
    value: &T,
    f: &mut std::fmt::Formatter,
) -> std::fmt::Result {
    write!(f, "<{} bytes>", value.as_ref().len())
}

/// The request an app made.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
#[serde(deny_unknown_fields)]
pub struct AppHttpRequest {
    /// `GET`, `POST`, `PUT`, `DELETE`, `PATCH` or `HEAD`.
    pub method: String,
    /// Absolute: `https://host[:port]/path?query`.
    pub url: String,
    pub headers: Vec<AppHttpHeader>,
    #[cfg_attr(feature = "typescript", ts(type = "number[]"))]
    #[debug(with = plaintext_debug_fmt)]
    pub body: Vec<u8>,
}

/// Why an `AppFetch` was not answered with a response. Closed: no upstream text reaches an
/// app through here.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
#[serde(tag = "kind", content = "detail")]
pub enum AppFetchRefusal {
    /// The SSRF policy refused it, before any connection was attempted. The same answer for
    /// every reason, so an app cannot map a network by which refusal it got.
    Policy,
    /// A limit was reached.
    Limit(AppFetchLimit),
    /// The request was allowed and could not be completed.
    Unavailable(AppFetchUnavailable),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum AppFetchLimit {
    /// More than the policy's requests per minute for this install and this account.
    Rate,
    /// The response body went past the policy's cap.
    ResponseBody,
    /// The policy's time budget ran out, redirects and waiting for credit included.
    Time,
    /// A `Credit` grant was zero or larger than one grant may be.
    Credit,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum AppFetchUnavailable {
    /// This agent was started without host egress configured.
    NotConfigured,
    /// The connection, TLS or HTTP exchange with the checked address failed.
    Network,
    /// The page cancelled the stream.
    Cancelled,
    /// The session is not this connection's, or is not signed in.
    Session,
    /// This connection already has a stream open under this request id.
    Duplicate,
}

/// A buffered `AppFetch`'s answer.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AppFetchResponse {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub status: u16,
    pub headers: Vec<AppHttpHeader>,
    #[cfg_attr(feature = "typescript", ts(type = "number[]"))]
    #[debug(with = plaintext_debug_fmt)]
    pub body: Vec<u8>,
    /// Redirects followed, each decided again by the policy.
    pub redirects: u8,
}

/// An `AppFetch` (or a stream it opened) ended without a response.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AppFetchFailure {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub refusal: AppFetchRefusal,
}

/// A streamed `AppFetch`'s status and headers. Chunks follow, one per credit.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AppFetchStreamHead {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    /// The `AppFetch`'s own request id: every event of the stream carries it.
    pub request_id: Option<Uuid>,
    pub status: u16,
    pub headers: Vec<AppHttpHeader>,
    pub redirects: u8,
}

/// One chunk of a streamed body, sent only against credit.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AppFetchStreamChunk {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    /// From 0, increasing by one.
    pub seq: u32,
    #[cfg_attr(feature = "typescript", ts(type = "number[]"))]
    #[debug(with = plaintext_debug_fmt)]
    pub bytes: Vec<u8>,
}

/// A streamed body ended whole.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AppFetchStreamEnd {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub body_bytes: u64,
}

/// What one install has moved through this agent for this account since the agent started.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AppNetUsage {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub install_id: String,
    /// Response bodies received.
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub bytes_in: u64,
    /// Request bodies sent, once per hop.
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub bytes_out: u64,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub requests: u64,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub refused: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Responses and requests are logged whole at debug level: an app's own credentials and
    /// bodies must not be in that log.
    #[test]
    fn credentials_and_bodies_never_print() {
        let request = AppHttpRequest {
            method: "POST".into(),
            url: "https://api.acme.com/".into(),
            headers: vec![AppHttpHeader {
                name: "authorization".into(),
                value: "Bearer hunter2".into(),
            }],
            body: b"top secret".to_vec(),
        };
        let printed = format!("{request:?}");
        assert!(!printed.contains("hunter2"), "{printed}");
        assert!(!printed.contains("116, 111, 112"), "{printed}");
        assert!(printed.contains("authorization"));
    }
}

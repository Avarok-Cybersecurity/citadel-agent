//! Internal HTTP over Citadel sessions: the frames an app's `InternalHttpClient` and another
//! member's `InternalHttpServer` exchange.
//!
//! A frame is one CBOR value, carried whole by one session message: over P2P, one
//! `SendReliable` message in the `citadel-ihttp/1` envelope; through the server, inside
//! `AppsRequest::IhttpSend` / `AppsResponse::IhttpFrame`. It is never textual HTTP/1.1: there
//! is no `Content-Length` or `Transfer-Encoding` to disagree about, so request smuggling has no
//! syntax to hide in, and the framing headers are refused outright (`validate`).
//!
//! A stream is a `RequestHead`, the request's `Body` chunks, `End`; then a `ResponseHead`, its
//! `Body` chunks, `End`. Either side may `Cancel`. Bodies move under credit: a receiver grants
//! `Credit { n }` for `n` more chunks, starting from `INITIAL_CREDIT`.
//!
//! There is no identity field. Who is calling is the session's knowledge (P2P) or stamped by
//! the kernel (relay), never asserted by an app.

mod codec;
mod validate;

#[cfg(test)]
mod tests;

pub use codec::{decode, encode};
pub use validate::{validate, FrameError};

use serde::{Deserialize, Serialize};
#[cfg(feature = "typescript")]
use ts_rs::TS;

/// The frame format this crate reads and writes. Any other `v` is refused.
pub const IHTTP_VERSION: u8 = 1;
/// The P2P envelope kind frames travel in.
pub const ENVELOPE_KIND: &str = "citadel-ihttp/1";
/// A request's path and query together.
pub const MAX_PATH_BYTES: usize = 2 * 1024;
pub const MAX_HEADERS: usize = 32;
/// One header's name and value together.
pub const MAX_HEADER_BYTES: usize = 1024;
pub const MAX_BODY_CHUNK_BYTES: usize = 64 * 1024;
/// The largest encoded frame `decode` reads: a full body chunk and its envelope.
pub const MAX_FRAME_BYTES: usize = MAX_BODY_CHUNK_BYTES + 4 * 1024;
pub const MAX_REQUEST_BODY_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_RESPONSE_BODY_BYTES: u64 = 16 * 1024 * 1024;
/// Chunks a sender may send before the first `Credit`.
pub const INITIAL_CREDIT: u32 = 4;
/// The most chunks one `Credit` frame grants.
pub const MAX_CREDIT_GRANT: u32 = 64;
pub const MAX_SCOPE_KEY_BYTES: usize = 256;
pub const STREAM_TIMEOUT_MS: u64 = 30_000;
/// A stream that keeps receiving credit may live this long.
pub const STREAMING_TIMEOUT_MS: u64 = 60_000;
pub const MAX_STREAMS_PER_CALLER: u32 = 8;
pub const MAX_STREAMS_PER_SERVER: u32 = 64;
pub const MAX_REQUESTS_PER_SECOND_PER_CALLER: u32 = 30;
/// Method-and-path patterns one app may serve.
pub const MAX_ROUTES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[serde(deny_unknown_fields)]
pub struct IhttpFrame {
    /// Always `IHTTP_VERSION`.
    pub v: u8,
    /// Per connection, chosen by the caller, increasing.
    pub stream: u64,
    pub kind: IhttpFrameKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[serde(deny_unknown_fields)]
pub enum IhttpFrameKind {
    RequestHead {
        method: IhttpMethod,
        /// Starts with `/`; no query, fragment, backslash or dot segment.
        path: String,
        query: Option<String>,
        headers: Vec<IhttpHeader>,
        /// None when the length is not known in advance.
        body_len: Option<u64>,
        /// The serving app; a server answers only for its own id.
        app_id: String,
        /// The install scope being addressed.
        scope_key: String,
    },
    ResponseHead {
        status: u16,
        headers: Vec<IhttpHeader>,
        /// None for a streamed response.
        body_len: Option<u64>,
    },
    Body {
        #[serde(with = "serde_bytes")]
        #[cfg_attr(feature = "typescript", ts(type = "Uint8Array"))]
        bytes: Vec<u8>,
    },
    End,
    Cancel {
        reason: CancelReason,
    },
    Credit {
        n: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS))]
pub enum IhttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
}

/// A lowercase name and a printable-ASCII value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[serde(deny_unknown_fields)]
pub struct IhttpHeader {
    pub name: String,
    pub value: String,
}

/// Why a stream ended early. Closed, so no free text crosses from one app to another here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS))]
pub enum CancelReason {
    /// The caller gave up.
    Caller,
    Timeout,
    /// A body went past its declared length or the cap.
    TooLarge,
    /// Too many streams open, for this caller or this server.
    Busy,
    RateLimited,
    /// No route matches the method and path.
    NoRoute,
    /// The server refused this caller.
    Refused,
    /// The serving app failed.
    Failed,
    /// The other end's app ended while the stream was open: it was closed (its zone unmounted,
    /// its page went away), uninstalled, or its host withdrew the service.
    Closed,
}

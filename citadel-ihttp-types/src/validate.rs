//! What a frame must be, beyond decoding: the version, the limits, and no framing headers.

use crate::{
    IhttpFrame, IhttpFrameKind, IhttpHeader, IHTTP_VERSION, MAX_BODY_CHUNK_BYTES, MAX_CREDIT_GRANT,
    MAX_FRAME_BYTES, MAX_HEADERS, MAX_HEADER_BYTES, MAX_PATH_BYTES, MAX_REQUEST_BODY_BYTES,
    MAX_RESPONSE_BODY_BYTES, MAX_SCOPE_KEY_BYTES,
};
use std::fmt;

/// Headers that frame or route a message. A frame carries its own framing, so a peer that
/// could set these could only be trying to confuse something downstream.
const FRAMING_HEADERS: [&str; 8] = [
    "connection",
    "content-length",
    "host",
    "keep-alive",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// More than `MAX_FRAME_BYTES` encoded.
    TooLarge,
    /// Not one well-formed frame: bad CBOR, an unknown kind or field, a missing field, or
    /// bytes after the frame.
    Malformed(String),
    Version(u8),
    Path,
    Query,
    Headers,
    Body,
    AppId,
    ScopeKey,
    Status,
    Credit,
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(why) => write!(f, "not an internal HTTP frame: {why}"),
            Self::Version(v) => write!(f, "internal HTTP frame version {v} is not {IHTTP_VERSION}"),
            other => write!(f, "internal HTTP frame refused: {other:?}"),
        }
    }
}

impl std::error::Error for FrameError {}

fn graphic(text: &str) -> bool {
    text.bytes().all(|b| b.is_ascii_graphic())
}

fn path_ok(path: &str) -> bool {
    path.starts_with('/')
        && graphic(path)
        && !path.contains(['?', '#', '\\'])
        && !path.contains("//")
        && path
            .split('/')
            .all(|segment| segment != "." && segment != "..")
}

fn header_ok(header: &IhttpHeader) -> bool {
    let name = header.name.as_str();
    name.len() + header.value.len() <= MAX_HEADER_BYTES
        && !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !FRAMING_HEADERS.contains(&name)
        && header
            .value
            .bytes()
            .all(|b| b == b'\t' || (0x20..=0x7e).contains(&b))
}

fn headers_ok(headers: &[IhttpHeader]) -> Result<(), FrameError> {
    if headers.len() > MAX_HEADERS || !headers.iter().all(header_ok) {
        return Err(FrameError::Headers);
    }
    Ok(())
}

/// Whether `frame` is one this version accepts, within every limit.
pub fn validate(frame: &IhttpFrame) -> Result<(), FrameError> {
    if frame.v != IHTTP_VERSION {
        return Err(FrameError::Version(frame.v));
    }
    match &frame.kind {
        IhttpFrameKind::RequestHead {
            path,
            query,
            headers,
            body_len,
            app_id,
            scope_key,
            ..
        } => {
            let query_len = query.as_ref().map_or(0, String::len);
            if !path_ok(path) || path.len() + query_len > MAX_PATH_BYTES {
                return Err(FrameError::Path);
            }
            if query
                .as_ref()
                .is_some_and(|q| !graphic(q) || q.contains('#'))
            {
                return Err(FrameError::Query);
            }
            headers_ok(headers)?;
            if body_len.is_some_and(|len| len > MAX_REQUEST_BODY_BYTES) {
                return Err(FrameError::Body);
            }
            citadel_app_id::validate_app_id(app_id).map_err(|_| FrameError::AppId)?;
            let key_ok = !scope_key.is_empty()
                && scope_key.len() <= MAX_SCOPE_KEY_BYTES
                && graphic(scope_key);
            if !key_ok {
                return Err(FrameError::ScopeKey);
            }
        }
        IhttpFrameKind::ResponseHead {
            status,
            headers,
            body_len,
        } => {
            if !(100..=599).contains(status) {
                return Err(FrameError::Status);
            }
            headers_ok(headers)?;
            if body_len.is_some_and(|len| len > MAX_RESPONSE_BODY_BYTES) {
                return Err(FrameError::Body);
            }
        }
        IhttpFrameKind::Body { bytes } => {
            if bytes.is_empty() || bytes.len() > MAX_BODY_CHUNK_BYTES {
                return Err(FrameError::Body);
            }
        }
        IhttpFrameKind::Credit { n } => {
            if *n == 0 || *n > MAX_CREDIT_GRANT {
                return Err(FrameError::Credit);
            }
        }
        IhttpFrameKind::End | IhttpFrameKind::Cancel { .. } => {}
    }
    Ok(())
}

/// The input-size gate `decode` applies before parsing anything.
pub(crate) fn within_frame_size(bytes: &[u8]) -> Result<(), FrameError> {
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge);
    }
    Ok(())
}

//! `Pointee` (`vfs-model.md` §4): what a file's bytes are. Every value here is within bounds.

use crate::cbor::{array, bytes_n, text, uint, CodecError, Value};
use crate::limits::{DOC_ID_MAX_BYTES, MIME_MAX_BYTES, VIRTUAL_PATH_MAX_BYTES};
use crate::principal::Principal;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pointee {
    /// Every directory.
    None,
    Standard {
        sha256: [u8; 32],
        len: u64,
        mime: String,
    },
    Revfs {
        owner: u64,
        host: Principal,
        virtual_path: String,
        len: u64,
        mime: String,
    },
    LiveDoc {
        doc_id: String,
    },
}

impl Pointee {
    pub fn to_value(&self) -> Value {
        match self {
            Self::None => Value::Array(vec![Value::Uint(0)]),
            Self::Standard { sha256, len, mime } => Value::Array(vec![
                Value::Uint(1),
                Value::bytes(sha256),
                Value::Uint(*len),
                Value::text(mime),
            ]),
            Self::Revfs {
                owner,
                host,
                virtual_path,
                len,
                mime,
            } => Value::Array(vec![
                Value::Uint(2),
                Value::Uint(*owner),
                host.to_value(),
                Value::text(virtual_path),
                Value::Uint(*len),
                Value::text(mime),
            ]),
            Self::LiveDoc { doc_id } => Value::Array(vec![Value::Uint(3), Value::text(doc_id)]),
        }
    }

    pub fn from_value(value: Value) -> Result<Self, CodecError> {
        let mut items = array(value, "pointee")?.into_iter();
        let mut next = || items.next().ok_or(CodecError::Expected("pointee"));
        let pointee = match uint(next()?, "pointee")? {
            0 => Self::None,
            1 => Self::Standard {
                sha256: bytes_n(next()?, "sha256")?,
                len: uint(next()?, "len")?,
                mime: text(next()?, "mime")?,
            },
            2 => Self::Revfs {
                owner: uint(next()?, "owner")?,
                host: Principal::from_value(next()?)?,
                virtual_path: text(next()?, "virtual_path")?,
                len: uint(next()?, "len")?,
                mime: text(next()?, "mime")?,
            },
            3 => Self::LiveDoc {
                doc_id: text(next()?, "doc_id")?,
            },
            _ => return Err(CodecError::OutOfRange("pointee")),
        };
        if items.next().is_some() {
            return Err(CodecError::Expected("pointee"));
        }
        pointee.check()?;
        Ok(pointee)
    }

    /// The field bounds of `vfs-model.md` §4.
    pub fn check(&self) -> Result<(), CodecError> {
        match self {
            Self::None => Ok(()),
            Self::Standard { mime, .. } => check_mime(mime),
            Self::Revfs {
                host,
                virtual_path,
                mime,
                ..
            } => {
                if !matches!(host, Principal::Server | Principal::Member { .. }) {
                    return Err(CodecError::OutOfRange("host"));
                }
                if virtual_path.is_empty() || virtual_path.len() > VIRTUAL_PATH_MAX_BYTES {
                    return Err(CodecError::OutOfRange("virtual_path"));
                }
                check_mime(mime)
            }
            Self::LiveDoc { doc_id } => {
                let shaped = !doc_id.is_empty()
                    && doc_id.len() <= DOC_ID_MAX_BYTES
                    && !doc_id
                        .chars()
                        .any(|c| c == '#' || c == ':' || c.is_whitespace());
                shaped.then_some(()).ok_or(CodecError::OutOfRange("doc_id"))
            }
        }
    }

    /// The plaintext length of a file's bytes (0 for a directory and a live document).
    pub fn byte_len(&self) -> u64 {
        match self {
            Self::Standard { len, .. } | Self::Revfs { len, .. } => *len,
            Self::None | Self::LiveDoc { .. } => 0,
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

/// `type/subtype` of RFC 6838 tokens, at most `MIME_MAX_BYTES`, or empty (the chat-attachment rule).
fn check_mime(mime: &str) -> Result<(), CodecError> {
    let token = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    let ok = mime.is_empty()
        || (mime.len() <= MIME_MAX_BYTES
            && mime
                .split_once('/')
                .is_some_and(|(kind, sub)| token(kind) && token(sub)));
    ok.then_some(()).ok_or(CodecError::OutOfRange("mime"))
}

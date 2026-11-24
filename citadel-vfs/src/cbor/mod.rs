//! RFC 8949 core deterministic CBOR for the five shapes the VFS bytes use (`vfs-ops.md` §2):
//! unsigned integers, byte strings, text, arrays and integer-keyed maps. Shortest heads,
//! definite lengths, map keys ascending without repeats; no floats, tags, `null`, booleans or
//! negative integers. The decoder refuses everything else, so every value has one encoding.

mod decode;
mod read;

pub use decode::decode;
pub use read::{array, array_n, bytes, bytes_n, text, uint, MapReader};

/// One decoded or to-be-encoded item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Uint(u64),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Value>),
    /// Integer-keyed; `encode` writes the entries in ascending key order.
    Map(Vec<(u64, Value)>),
}

/// Why bytes are not canonical CBOR of the expected shape.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CodecError {
    Truncated,
    /// An integer or length written with a longer head than it needs.
    NonShortest,
    /// An indefinite length, or a reserved additional-information value.
    Indefinite,
    /// A float, tag, simple value (`null`, booleans) or negative integer.
    Unsupported,
    BadUtf8,
    /// Map keys out of ascending order, or repeated.
    Unsorted,
    /// A map key that is not an unsigned integer.
    KeyType,
    Trailing,
    TooDeep,
    /// The item is not of the expected major type or arity (`what` names the field).
    Expected(&'static str),
    MissingKey(u64),
    UnknownKey(u64),
    /// A field outside its frozen bounds (`what` names it).
    OutOfRange(&'static str),
    /// Longer than the protocol allows.
    TooLarge,
}

impl core::fmt::Display for CodecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "not canonical VFS CBOR: {self:?}")
    }
}

impl std::error::Error for CodecError {}

pub(crate) const UINT: u8 = 0;
pub(crate) const BYTES: u8 = 2;
pub(crate) const TEXT: u8 = 3;
pub(crate) const ARRAY: u8 = 4;
pub(crate) const MAP: u8 = 5;

impl Value {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        match self {
            Self::Uint(n) => head(out, UINT, *n),
            Self::Bytes(b) => {
                head(out, BYTES, b.len() as u64);
                out.extend_from_slice(b);
            }
            Self::Text(s) => {
                head(out, TEXT, s.len() as u64);
                out.extend_from_slice(s.as_bytes());
            }
            Self::Array(items) => {
                head(out, ARRAY, items.len() as u64);
                for item in items {
                    item.encode_into(out);
                }
            }
            Self::Map(entries) => {
                let mut sorted: Vec<&(u64, Value)> = entries.iter().collect();
                sorted.sort_by_key(|(key, _)| *key);
                head(out, MAP, sorted.len() as u64);
                for (key, value) in sorted {
                    head(out, UINT, *key);
                    value.encode_into(out);
                }
            }
        }
    }

    pub fn text(s: &str) -> Self {
        Self::Text(s.to_string())
    }

    pub fn bytes(b: &[u8]) -> Self {
        Self::Bytes(b.to_vec())
    }
}

/// The shortest head for `n` (RFC 8949 §4.2.1).
pub(crate) fn head(out: &mut Vec<u8>, major: u8, n: u64) {
    let major = major << 5;
    if n < 24 {
        out.push(major | n as u8);
    } else if let Ok(n) = u8::try_from(n) {
        out.extend_from_slice(&[major | 24, n]);
    } else if let Ok(n) = u16::try_from(n) {
        out.push(major | 25);
        out.extend_from_slice(&n.to_be_bytes());
    } else if let Ok(n) = u32::try_from(n) {
        out.push(major | 26);
        out.extend_from_slice(&n.to_be_bytes());
    } else {
        out.push(major | 27);
        out.extend_from_slice(&n.to_be_bytes());
    }
}

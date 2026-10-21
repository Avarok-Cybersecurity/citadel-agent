//! Raw bytes that travel as one CBOR byte string (major type 2), never an array of integers.

use alloc::vec::Vec;
use core::fmt;
use serde::de::{Error, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Opaque bytes. On the TypeScript side a `Uint8Array`.
#[derive(Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(type = "Uint8Array"))]
pub struct Bytes(pub Vec<u8>);

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bytes(len={})", self.0.len())
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(v: Vec<u8>) -> Self {
        Bytes(v)
    }
}

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&self.0)
    }
}

struct BytesVisitor;

impl<'de> Visitor<'de> for BytesVisitor {
    type Value = Bytes;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a byte string")
    }

    fn visit_bytes<E: Error>(self, v: &[u8]) -> Result<Bytes, E> {
        Ok(Bytes(v.to_vec()))
    }

    fn visit_byte_buf<E: Error>(self, v: Vec<u8>) -> Result<Bytes, E> {
        Ok(Bytes(v))
    }

    /// An encoder that writes a `Uint8Array` as an array of small integers still decodes.
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Bytes, A::Error> {
        let mut out = Vec::new();
        while let Some(b) = seq.next_element::<u8>()? {
            out.push(b);
        }
        Ok(Bytes(out))
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Bytes, D::Error> {
        d.deserialize_byte_buf(BytesVisitor)
    }
}

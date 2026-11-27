//! Byte fields: one encoding on the agent's wire, a typed array on the page.
//!
//! `#[serde(with = "wire_bytes")]` (and `wire_bytes::option`, `::seq`, `::map`) makes a byte
//! field serialize through `serialize_bytes`. serde_json (the agent socket) writes that as the
//! same array of numbers a plain `Vec<u8>` writes, and bincode2 (TCP) as the same length plus
//! raw bytes, so neither wire changes and agents of either build keep talking. serde-wasm-bindgen
//! turns it into a `Uint8Array` (one copy) instead of a JS array of numbers.
//!
//! Decoding accepts exactly what a plain `Vec<u8>` accepted, plus an owned byte buffer:
//! an array of numbers (serde_json, a JS `Array`), or a byte buffer (bincode2, a JS
//! `Uint8Array`/`ArrayBuffer`). A JSON string is refused, as before: serde_json hands a string
//! to `visit_bytes`/`visit_borrowed_bytes`, which this visitor does not accept.
use serde::de::{Deserialize, Deserializer, SeqAccess, Visitor};
use serde::ser::{Serialize, Serializer};
use std::collections::HashMap;
use std::fmt;

/// The most a byte array's own length hint may preallocate (the rest grows as it arrives).
const PREALLOCATE_CAP: usize = 64 * 1024;

pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_bytes(bytes)
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    deserializer.deserialize_byte_buf(BytesVisitor)
}

struct BytesVisitor;

impl<'de> Visitor<'de> for BytesVisitor {
    type Value = Vec<u8>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an array of bytes")
    }

    fn visit_byte_buf<E>(self, bytes: Vec<u8>) -> Result<Vec<u8>, E> {
        Ok(bytes)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u8>, A::Error> {
        let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(PREALLOCATE_CAP));
        while let Some(byte) = seq.next_element::<u8>()? {
            out.push(byte);
        }
        Ok(out)
    }
}

/// One byte field, for the containers below.
struct Ref<'a>(&'a [u8]);

impl Serialize for Ref<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self.0)
    }
}

struct Owned(Vec<u8>);

impl<'de> Deserialize<'de> for Owned {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize(deserializer).map(Owned)
    }
}

/// `Option<Vec<u8>>`. A struct field also needs `#[serde(default)]` to keep reading a missing
/// field as `None`, which serde's derive does by itself only without `with`.
pub mod option {
    use super::*;

    pub fn serialize<S: Serializer>(
        bytes: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(bytes) => serializer.serialize_some(&Ref(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        Ok(Option::<Owned>::deserialize(deserializer)?.map(|owned| owned.0))
    }
}

/// `Vec<Vec<u8>>`.
pub mod seq {
    use super::*;

    #[allow(clippy::ptr_arg)] // serde's `with` hands the field over as `&Vec`.
    pub fn serialize<S: Serializer>(list: &Vec<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(list.iter().map(|bytes| Ref(bytes)))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<Vec<u8>>, D::Error> {
        Ok(Vec::<Owned>::deserialize(deserializer)?
            .into_iter()
            .map(|owned| owned.0)
            .collect())
    }
}

/// `HashMap<String, Vec<u8>>`.
pub mod map {
    use super::*;

    pub fn serialize<S: Serializer>(
        map: &HashMap<String, Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_map(map.iter().map(|(key, bytes)| (key, Ref(bytes))))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<HashMap<String, Vec<u8>>, D::Error> {
        Ok(HashMap::<String, Owned>::deserialize(deserializer)?
            .into_iter()
            .map(|(key, owned)| (key, owned.0))
            .collect())
    }
}

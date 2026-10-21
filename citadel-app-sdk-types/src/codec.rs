//! The CBOR codec both sides use: one encoder, one strict decoder (trailing bytes refused).

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Why bytes did not decode as the expected message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeError(pub String);

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "not the expected CBOR message: {}", self.0)
    }
}

/// CBOR of `value`. Infallible for the ABI's types (writing into a `Vec` cannot fail).
pub fn encode<T: Serialize + ?Sized>(value: &T) -> Vec<u8> {
    let mut out = Vec::new();
    match ciborium::into_writer(value, &mut out) {
        Ok(()) => out,
        // A Vec writer has no I/O error, and every ABI type is serializable: this is unreachable
        // for them, and an encoder bug must not be mistaken for an empty message.
        Err(e) => panic!("CBOR encoding of an ABI value failed: {e:?}"),
    }
}

/// Exactly one CBOR `T` spanning all of `bytes`.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, DecodeError> {
    let mut rest = bytes;
    let value = ciborium::from_reader(&mut rest).map_err(|e| DecodeError(e.to_string()))?;
    if !rest.is_empty() {
        return Err(DecodeError(alloc::format!(
            "{} trailing byte(s)",
            rest.len()
        )));
    }
    Ok(value)
}

//! Typed reads over a decoded `Value`: every field required, every unknown key refused.

use super::{CodecError, Value};

/// The entries of one map, taken by key; `finish` refuses any key nobody asked for.
pub struct MapReader {
    entries: Vec<(u64, Option<Value>)>,
}

impl MapReader {
    pub fn new(value: Value, what: &'static str) -> Result<Self, CodecError> {
        match value {
            Value::Map(entries) => Ok(Self {
                entries: entries.into_iter().map(|(k, v)| (k, Some(v))).collect(),
            }),
            _ => Err(CodecError::Expected(what)),
        }
    }

    pub fn take(&mut self, key: u64) -> Result<Value, CodecError> {
        self.entries
            .iter_mut()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v.take())
            .ok_or(CodecError::MissingKey(key))
    }

    pub fn finish(self) -> Result<(), CodecError> {
        match self.entries.iter().find(|(_, v)| v.is_some()) {
            Some((key, _)) => Err(CodecError::UnknownKey(*key)),
            None => Ok(()),
        }
    }
}

pub fn uint(value: Value, what: &'static str) -> Result<u64, CodecError> {
    match value {
        Value::Uint(n) => Ok(n),
        _ => Err(CodecError::Expected(what)),
    }
}

pub fn bytes(value: Value, what: &'static str) -> Result<Vec<u8>, CodecError> {
    match value {
        Value::Bytes(b) => Ok(b),
        _ => Err(CodecError::Expected(what)),
    }
}

/// A byte string of exactly `N` bytes.
pub fn bytes_n<const N: usize>(value: Value, what: &'static str) -> Result<[u8; N], CodecError> {
    bytes(value, what)?
        .try_into()
        .map_err(|_| CodecError::OutOfRange(what))
}

pub fn text(value: Value, what: &'static str) -> Result<String, CodecError> {
    match value {
        Value::Text(s) => Ok(s),
        _ => Err(CodecError::Expected(what)),
    }
}

pub fn array(value: Value, what: &'static str) -> Result<Vec<Value>, CodecError> {
    match value {
        Value::Array(items) => Ok(items),
        _ => Err(CodecError::Expected(what)),
    }
}

/// An array of exactly `N` items.
pub fn array_n<const N: usize>(value: Value, what: &'static str) -> Result<[Value; N], CodecError> {
    array(value, what)?
        .try_into()
        .map_err(|_| CodecError::Expected(what))
}

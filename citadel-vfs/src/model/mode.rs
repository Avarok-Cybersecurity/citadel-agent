//! `Mode` (`vfs-model.md` §5): nine bits, owner / members / outsiders, as on Linux.

use crate::cbor::{uint, CodecError, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mode(u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Bit {
    R,
    W,
    X,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Triple {
    Owner,
    Members,
    Outsiders,
}

impl Mode {
    /// Only the low nine bits may be set.
    pub fn new(bits: u16) -> Option<Self> {
        (bits <= 0o777).then_some(Self(bits))
    }

    pub fn bits(self) -> u16 {
        self.0
    }

    pub fn has(self, triple: Triple, bit: Bit) -> bool {
        let shift = match triple {
            Triple::Owner => 6,
            Triple::Members => 3,
            Triple::Outsiders => 0,
        };
        let mask = match bit {
            Bit::R => 4,
            Bit::W => 2,
            Bit::X => 1,
        };
        (self.0 >> shift) & mask != 0
    }

    pub fn to_value(self) -> Value {
        Value::Uint(u64::from(self.0))
    }

    pub fn from_value(value: Value) -> Result<Self, CodecError> {
        u16::try_from(uint(value, "mode")?)
            .ok()
            .and_then(Self::new)
            .ok_or(CodecError::OutOfRange("mode"))
    }
}

//! `Header` (`vfs-ops.md` §2): what a signature covers. It commits to the body by hash.

use crate::cbor::{bytes, bytes_n, decode, text, uint, CodecError, MapReader, Value};
use crate::inode::InodeId;
use crate::op::OpKind;
use crate::principal::Principal;

pub const HEADER_VERSION: u64 = 1;
/// `lamport` is 1..=2^63 - 1.
pub const LAMPORT_MAX: u64 = (1 << 63) - 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub volume: String,
    pub op_id: [u8; 16],
    pub author: Principal,
    pub lamport: u64,
    /// The `op_hash` of this author's previous op in this volume; `None` for its first.
    pub prev: Option<[u8; 32]>,
    pub kind: OpKind,
    pub target: InodeId,
    pub body_hash: [u8; 32],
    /// The author's wall clock; shown to people, never orders anything.
    pub at_ms: u64,
}

/// Why header bytes are refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderError {
    Codec(CodecError),
    /// `v != 1` or a kind this build does not know (`vfs-ops.md` R12): never guessed at.
    UpdateNeeded,
}

impl From<CodecError> for HeaderError {
    fn from(e: CodecError) -> Self {
        Self::Codec(e)
    }
}

impl Header {
    pub fn to_value(&self) -> Value {
        Value::Map(vec![
            (1, Value::Uint(HEADER_VERSION)),
            (2, Value::text(&self.volume)),
            (3, Value::bytes(&self.op_id)),
            (4, self.author.to_value()),
            (5, Value::Uint(self.lamport)),
            (
                6,
                Value::bytes(self.prev.as_ref().map_or(&[][..], |p| &p[..])),
            ),
            (7, Value::Uint(self.kind.code())),
            (8, Value::bytes(&self.target.0)),
            (9, Value::bytes(&self.body_hash)),
            (10, Value::Uint(self.at_ms)),
        ])
    }

    pub fn encode(&self) -> Vec<u8> {
        self.to_value().encode()
    }

    pub fn decode(raw: &[u8]) -> Result<Self, HeaderError> {
        let mut map = MapReader::new(decode(raw)?, "header")?;
        if uint(map.take(1)?, "v")? != HEADER_VERSION {
            return Err(HeaderError::UpdateNeeded);
        }
        let kind =
            OpKind::from_code(uint(map.take(7)?, "kind")?).ok_or(HeaderError::UpdateNeeded)?;
        let lamport = uint(map.take(5)?, "lamport")?;
        if !(1..=LAMPORT_MAX).contains(&lamport) {
            return Err(CodecError::OutOfRange("lamport").into());
        }
        let prev = match bytes(map.take(6)?, "prev")? {
            empty if empty.is_empty() => None,
            full => Some(
                full.try_into()
                    .map_err(|_| CodecError::OutOfRange("prev"))?,
            ),
        };
        let header = Self {
            volume: text(map.take(2)?, "volume")?,
            op_id: bytes_n(map.take(3)?, "op_id")?,
            author: Principal::from_value(map.take(4)?)?,
            lamport,
            prev,
            kind,
            target: InodeId(bytes_n(map.take(8)?, "target")?),
            body_hash: bytes_n(map.take(9)?, "body_hash")?,
            at_ms: uint(map.take(10)?, "at_ms")?,
        };
        map.finish()?;
        Ok(header)
    }

    /// `(lamport, author bytes, op_id)`: the position in a Merge volume.
    pub fn stamp(&self) -> Stamp {
        Stamp {
            lamport: self.lamport,
            author: self.author.clone(),
            op_id: self.op_id,
        }
    }
}

/// The stamp (`vfs-ops.md` R1). Orders by `lamport`, then the author's canonical bytes, then `op_id`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stamp {
    pub lamport: u64,
    pub author: Principal,
    pub op_id: [u8; 16],
}

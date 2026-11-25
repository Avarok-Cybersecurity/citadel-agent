//! `Principal` (`vfs-model.md` §3): who authors an op or owns an entry.

use crate::cbor::{array, bytes_n, text, uint, CodecError, Value};

/// Characters of a fingerprint (`citadel-identity`'s base32, 80 bits).
pub const KEY_FP_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Principal {
    Server,
    Member {
        cid: u64,
    },
    /// An outsider that claimed a link with an invitation key.
    LinkGuest {
        link_id: [u8; 16],
        key_fp: String,
    },
}

impl Principal {
    pub const fn member(cid: u64) -> Self {
        Self::Member { cid }
    }

    pub fn cid(&self) -> Option<u64> {
        match self {
            Self::Member { cid } => Some(*cid),
            _ => None,
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            Self::Server => Value::Array(vec![Value::Uint(0)]),
            Self::Member { cid } => Value::Array(vec![Value::Uint(1), Value::Uint(*cid)]),
            Self::LinkGuest { link_id, key_fp } => Value::Array(vec![
                Value::Uint(2),
                Value::bytes(link_id),
                Value::text(key_fp),
            ]),
        }
    }

    pub fn from_value(value: Value) -> Result<Self, CodecError> {
        let mut items = array(value, "principal")?.into_iter();
        let tag = uint(
            items.next().ok_or(CodecError::Expected("principal"))?,
            "principal",
        )?;
        let principal = match tag {
            0 => Self::Server,
            1 => Self::Member {
                cid: uint(
                    items.next().ok_or(CodecError::Expected("principal"))?,
                    "cid",
                )?,
            },
            2 => {
                let link_id = bytes_n(
                    items.next().ok_or(CodecError::Expected("principal"))?,
                    "link_id",
                )?;
                let key_fp = text(
                    items.next().ok_or(CodecError::Expected("principal"))?,
                    "key_fp",
                )?;
                if !is_key_fp(&key_fp) {
                    return Err(CodecError::OutOfRange("key_fp"));
                }
                Self::LinkGuest { link_id, key_fp }
            }
            _ => return Err(CodecError::OutOfRange("principal")),
        };
        match items.next() {
            Some(_) => Err(CodecError::Expected("principal")),
            None => Ok(principal),
        }
    }

    /// The canonical bytes, which also order principals (`Server < Member < LinkGuest`).
    pub fn encode(&self) -> Vec<u8> {
        self.to_value().encode()
    }
}

impl PartialOrd for Principal {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Principal {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.encode().cmp(&other.encode())
    }
}

/// 16 characters of lowercase RFC 4648 base32, as `citadel_identity::fingerprint` writes.
pub fn is_key_fp(s: &str) -> bool {
    s.len() == KEY_FP_LEN
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
}

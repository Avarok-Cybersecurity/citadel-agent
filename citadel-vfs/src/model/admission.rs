//! Admissions of outsiders (`vfs-model.md` §4, `vfs-outsiders.md` §1–§3).

use crate::cbor::{array, bytes, bytes_n, text, uint, CodecError, Value};
use crate::limits::MAX_LINK_CLAIMS;
use crate::principal::is_key_fp;
use citadel_identity::{PUBLIC_KEY_LEN, SIGNATURE_LEN};

/// Who an `Admit` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admitted {
    /// Kind A: a workspace member who is not in this volume, and its identity key's fingerprint.
    Member { cid: u64, id_fp: String },
    /// Kinds C and D: a link, its public key and how many guests may claim it (1..=50).
    Link {
        link_id: [u8; 16],
        link_pk: Vec<u8>,
        max_claims: u64,
    },
}

/// The key an admission is stored under: `[1, cid]` or `[2, link_id]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AdmittedKey {
    Member { cid: u64 },
    Link { link_id: [u8; 16] },
}

/// `[max_bytes, max_entries, max_ops_per_min]`; `[0, 0, 0]` admits reading only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    pub max_bytes: u64,
    pub max_entries: u64,
    pub max_ops_per_min: u64,
}

/// What a `Claim` states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimWhat {
    /// "This key is a guest of this link", with the link key's signature over it.
    Key { key_pk: Vec<u8>, link_sig: Vec<u8> },
    /// "My access passes to this identity."
    Handover { cid: u64, id_fp: String },
}

fn next(items: &mut std::vec::IntoIter<Value>, what: &'static str) -> Result<Value, CodecError> {
    items.next().ok_or(CodecError::Expected(what))
}

fn done(mut items: std::vec::IntoIter<Value>, what: &'static str) -> Result<(), CodecError> {
    items
        .next()
        .map_or(Ok(()), |_| Err(CodecError::Expected(what)))
}

fn fp(value: Value, what: &'static str) -> Result<String, CodecError> {
    let s = text(value, what)?;
    is_key_fp(&s)
        .then_some(s)
        .ok_or(CodecError::OutOfRange(what))
}

fn exact(value: Value, len: usize, what: &'static str) -> Result<Vec<u8>, CodecError> {
    let b = bytes(value, what)?;
    (b.len() == len)
        .then_some(b)
        .ok_or(CodecError::OutOfRange(what))
}

impl Admitted {
    pub fn key(&self) -> AdmittedKey {
        match self {
            Self::Member { cid, .. } => AdmittedKey::Member { cid: *cid },
            Self::Link { link_id, .. } => AdmittedKey::Link { link_id: *link_id },
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            Self::Member { cid, id_fp } => {
                Value::Array(vec![Value::Uint(1), Value::Uint(*cid), Value::text(id_fp)])
            }
            Self::Link {
                link_id,
                link_pk,
                max_claims,
            } => Value::Array(vec![
                Value::Uint(2),
                Value::bytes(link_id),
                Value::bytes(link_pk),
                Value::Uint(*max_claims),
            ]),
        }
    }

    pub fn from_value(value: Value) -> Result<Self, CodecError> {
        let mut items = array(value, "admitted")?.into_iter();
        let admitted = match uint(next(&mut items, "admitted")?, "admitted")? {
            1 => Self::Member {
                cid: uint(next(&mut items, "admitted")?, "cid")?,
                id_fp: fp(next(&mut items, "admitted")?, "id_fp")?,
            },
            2 => {
                let link_id = bytes_n(next(&mut items, "admitted")?, "link_id")?;
                let link_pk = exact(next(&mut items, "admitted")?, PUBLIC_KEY_LEN, "link_pk")?;
                let max_claims = uint(next(&mut items, "admitted")?, "max_claims")?;
                if !(1..=MAX_LINK_CLAIMS).contains(&max_claims) {
                    return Err(CodecError::OutOfRange("max_claims"));
                }
                Self::Link {
                    link_id,
                    link_pk,
                    max_claims,
                }
            }
            _ => return Err(CodecError::OutOfRange("admitted")),
        };
        done(items, "admitted")?;
        Ok(admitted)
    }
}

impl AdmittedKey {
    pub fn to_value(&self) -> Value {
        match self {
            Self::Member { cid } => Value::Array(vec![Value::Uint(1), Value::Uint(*cid)]),
            Self::Link { link_id } => Value::Array(vec![Value::Uint(2), Value::bytes(link_id)]),
        }
    }

    pub fn from_value(value: Value) -> Result<Self, CodecError> {
        let mut items = array(value, "who")?.into_iter();
        let key = match uint(next(&mut items, "who")?, "who")? {
            1 => Self::Member {
                cid: uint(next(&mut items, "who")?, "cid")?,
            },
            2 => Self::Link {
                link_id: bytes_n(next(&mut items, "who")?, "link_id")?,
            },
            _ => return Err(CodecError::OutOfRange("who")),
        };
        done(items, "who")?;
        Ok(key)
    }
}

impl Caps {
    pub fn to_value(&self) -> Value {
        Value::Array(vec![
            Value::Uint(self.max_bytes),
            Value::Uint(self.max_entries),
            Value::Uint(self.max_ops_per_min),
        ])
    }

    pub fn from_value(value: Value) -> Result<Self, CodecError> {
        let [a, b, c] = crate::cbor::array_n::<3>(value, "caps")?;
        Ok(Self {
            max_bytes: uint(a, "caps")?,
            max_entries: uint(b, "caps")?,
            max_ops_per_min: uint(c, "caps")?,
        })
    }
}

impl ClaimWhat {
    pub fn to_value(&self) -> Value {
        match self {
            Self::Key { key_pk, link_sig } => Value::Array(vec![
                Value::Uint(1),
                Value::bytes(key_pk),
                Value::bytes(link_sig),
            ]),
            Self::Handover { cid, id_fp } => {
                Value::Array(vec![Value::Uint(2), Value::Uint(*cid), Value::text(id_fp)])
            }
        }
    }

    pub fn from_value(value: Value) -> Result<Self, CodecError> {
        let mut items = array(value, "what")?.into_iter();
        let what = match uint(next(&mut items, "what")?, "what")? {
            1 => Self::Key {
                key_pk: exact(next(&mut items, "what")?, PUBLIC_KEY_LEN, "key_pk")?,
                link_sig: exact(next(&mut items, "what")?, SIGNATURE_LEN, "link_sig")?,
            },
            2 => Self::Handover {
                cid: uint(next(&mut items, "what")?, "cid")?,
                id_fp: fp(next(&mut items, "what")?, "id_fp")?,
            },
            _ => return Err(CodecError::OutOfRange("what")),
        };
        done(items, "what")?;
        Ok(what)
    }
}

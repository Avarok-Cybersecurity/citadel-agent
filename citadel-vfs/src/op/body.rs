//! `Body` by kind (`vfs-ops.md` §2). Decoding is by the header's kind; every value is in bounds.

use crate::cbor::{array, bytes, bytes_n, decode, text, uint, CodecError, MapReader, Value};
use crate::inode::InodeId;
use crate::model::{Admitted, AdmittedKey, Caps, ClaimWhat, EntryKind, Mode, Pointee};
use crate::name::Name;
use crate::op::OpKind;
use crate::principal::Principal;

/// Per author principal of a dismissed outsider, the `op_hash` of its last accepted op.
pub type Lasts = Vec<(Principal, Option<[u8; 32]>)>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Create {
        kind: EntryKind,
        parent: InodeId,
        name: Name,
        pointee: Pointee,
        mode: Mode,
        owner: Principal,
    },
    Place {
        parent: InodeId,
        name: Name,
    },
    SetPointee {
        pointee: Pointee,
    },
    Remove,
    Restore,
    Purge,
    Chmod {
        mode: Mode,
    },
    Chown {
        owner: Principal,
    },
    /// `expires_ms` 0 = until dismissed.
    Admit {
        who: Admitted,
        expires_ms: u64,
        caps: Caps,
    },
    /// `lasts`: per author principal of the outsider, the `op_hash` of its last accepted op.
    Dismiss {
        who: AdmittedKey,
        lasts: Lasts,
    },
    Claim {
        link_id: [u8; 16],
        what: ClaimWhat,
    },
}

impl Body {
    pub fn kind(&self) -> OpKind {
        match self {
            Self::Create { .. } => OpKind::Create,
            Self::Place { .. } => OpKind::Place,
            Self::SetPointee { .. } => OpKind::SetPointee,
            Self::Remove => OpKind::Remove,
            Self::Restore => OpKind::Restore,
            Self::Purge => OpKind::Purge,
            Self::Chmod { .. } => OpKind::Chmod,
            Self::Chown { .. } => OpKind::Chown,
            Self::Admit { .. } => OpKind::Admit,
            Self::Dismiss { .. } => OpKind::Dismiss,
            Self::Claim { .. } => OpKind::Claim,
        }
    }

    pub fn to_value(&self) -> Value {
        let id = |i: &InodeId| Value::bytes(&i.0);
        let entries = match self {
            Self::Create {
                kind,
                parent,
                name,
                pointee,
                mode,
                owner,
            } => vec![
                (1, Value::Uint(*kind as u64)),
                (2, id(parent)),
                (3, Value::text(name.as_str())),
                (4, pointee.to_value()),
                (5, mode.to_value()),
                (6, owner.to_value()),
            ],
            Self::Place { parent, name } => vec![(1, id(parent)), (2, Value::text(name.as_str()))],
            Self::SetPointee { pointee } => vec![(1, pointee.to_value())],
            Self::Remove | Self::Restore | Self::Purge => vec![],
            Self::Chmod { mode } => vec![(1, mode.to_value())],
            Self::Chown { owner } => vec![(1, owner.to_value())],
            Self::Admit {
                who,
                expires_ms,
                caps,
            } => {
                vec![
                    (1, who.to_value()),
                    (2, Value::Uint(*expires_ms)),
                    (3, caps.to_value()),
                ]
            }
            Self::Dismiss { who, lasts } => {
                let lasts = lasts
                    .iter()
                    .map(|(author, hash)| {
                        let hash = hash.as_ref().map_or(&[][..], |h| &h[..]);
                        Value::Array(vec![author.to_value(), Value::bytes(hash)])
                    })
                    .collect();
                vec![(1, who.to_value()), (2, Value::Array(lasts))]
            }
            Self::Claim { link_id, what } => vec![(1, Value::bytes(link_id)), (2, what.to_value())],
        };
        Value::Map(entries)
    }

    pub fn encode(&self) -> Vec<u8> {
        self.to_value().encode()
    }

    pub fn decode(kind: OpKind, raw: &[u8]) -> Result<Self, CodecError> {
        let mut map = MapReader::new(decode(raw)?, "body")?;
        let body = match kind {
            OpKind::Create => Self::Create {
                kind: EntryKind::from_code(uint(map.take(1)?, "kind")?)
                    .ok_or(CodecError::OutOfRange("kind"))?,
                parent: inode(map.take(2)?)?,
                name: name(map.take(3)?)?,
                pointee: Pointee::from_value(map.take(4)?)?,
                mode: Mode::from_value(map.take(5)?)?,
                owner: Principal::from_value(map.take(6)?)?,
            },
            OpKind::Place => Self::Place {
                parent: inode(map.take(1)?)?,
                name: name(map.take(2)?)?,
            },
            OpKind::SetPointee => Self::SetPointee {
                pointee: Pointee::from_value(map.take(1)?)?,
            },
            OpKind::Remove => Self::Remove,
            OpKind::Restore => Self::Restore,
            OpKind::Purge => Self::Purge,
            OpKind::Chmod => Self::Chmod {
                mode: Mode::from_value(map.take(1)?)?,
            },
            OpKind::Chown => Self::Chown {
                owner: Principal::from_value(map.take(1)?)?,
            },
            OpKind::Admit => Self::Admit {
                who: Admitted::from_value(map.take(1)?)?,
                expires_ms: uint(map.take(2)?, "expires_ms")?,
                caps: Caps::from_value(map.take(3)?)?,
            },
            OpKind::Dismiss => Self::Dismiss {
                who: AdmittedKey::from_value(map.take(1)?)?,
                lasts: lasts(map.take(2)?)?,
            },
            OpKind::Claim => Self::Claim {
                link_id: bytes_n(map.take(1)?, "link_id")?,
                what: ClaimWhat::from_value(map.take(2)?)?,
            },
        };
        map.finish()?;
        Ok(body)
    }
}

fn inode(value: Value) -> Result<InodeId, CodecError> {
    Ok(InodeId(bytes_n(value, "inode")?))
}

fn name(value: Value) -> Result<Name, CodecError> {
    Name::new(&text(value, "name")?).map_err(|_| CodecError::OutOfRange("name"))
}

fn lasts(value: Value) -> Result<Lasts, CodecError> {
    array(value, "lasts")?
        .into_iter()
        .map(|item| {
            let [author, hash] = crate::cbor::array_n::<2>(item, "lasts")?;
            let hash = match bytes(hash, "lasts")? {
                empty if empty.is_empty() => None,
                full => Some(
                    full.try_into()
                        .map_err(|_| CodecError::OutOfRange("lasts"))?,
                ),
            };
            Ok((Principal::from_value(author)?, hash))
        })
        .collect()
}

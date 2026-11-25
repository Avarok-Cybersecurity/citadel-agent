//! `TreeSnapshot` (`vfs-wire.md` §3.3): the kernel's statement of a volume's tree, for painting
//! before the log is pulled. It holds no admissions and no positions: a tree restored from it
//! ranks equal names by id, and authorizes no outsider, until the log is folded.

use crate::cbor::{
    array, array_n, bytes_n, decode as cbor_decode, text, uint, CodecError, MapReader, Value,
};
use crate::inode::InodeId;
use crate::model::{EntryKind, Mode, Pointee, Stamped};
use crate::name::Name;
use crate::op::Stamp;
use crate::principal::Principal;
use crate::reduce::{Entry, Place, Position, Reg, RestoreError, Tree, Won};
use crate::volume::VolumeId;
use std::collections::{BTreeMap, BTreeSet};

pub const SNAPSHOT_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    Codec(CodecError),
    UpdateNeeded,
    Volume,
    Name,
    Tree(RestoreError),
}

impl From<CodecError> for SnapshotError {
    fn from(e: CodecError) -> Self {
        Self::Codec(e)
    }
}

fn stamp(s: &Stamp) -> Value {
    Value::Array(vec![
        Value::Uint(s.lamport),
        s.author.to_value(),
        Value::bytes(&s.op_id),
    ])
}

fn with(value: Value, won: &Won) -> Value {
    Value::Array(vec![value, stamp(&won.stamp)])
}

fn entry(e: &Entry) -> Value {
    let place = &e.place;
    let removal = match e
        .removal
        .as_ref()
        .and_then(|r| r.value.as_ref().map(|s| (s, &r.won)))
    {
        Some((s, won)) => vec![s.by.to_value(), Value::Uint(s.at_ms), stamp(&won.stamp)],
        None => Vec::new(),
    };
    Value::Map(vec![
        (1, Value::bytes(&e.id.0)),
        (2, Value::Uint(e.kind as u64)),
        (
            3,
            Value::Array(vec![e.created.by.to_value(), Value::Uint(e.created.at_ms)]),
        ),
        (
            4,
            Value::Array(vec![
                Value::bytes(&place.value.parent.0),
                Value::text(place.value.name.as_str()),
                stamp(&place.won.stamp),
            ]),
        ),
        (5, with(e.pointee.value.to_value(), &e.pointee.won)),
        (6, with(e.owner.value.to_value(), &e.owner.won)),
        (7, with(e.mode.value.to_value(), &e.mode.won)),
        (8, Value::Array(removal)),
    ])
}

/// The snapshot of `tree` as of `upto_seq`.
pub fn encode(tree: &Tree, upto_seq: u64) -> Vec<u8> {
    let entries = tree.entries().map(entry).collect();
    let purged = tree.purged().iter().map(|id| Value::bytes(&id.0)).collect();
    Value::Map(vec![
        (1, Value::Uint(SNAPSHOT_VERSION)),
        (2, Value::text(&tree.volume.to_string())),
        (3, Value::Uint(upto_seq)),
        (4, Value::Array(entries)),
        (5, Value::Array(purged)),
    ])
    .encode()
}

fn read_stamp(v: Value) -> Result<Won, SnapshotError> {
    let [lamport, author, op_id] = array_n::<3>(v, "stamp")?;
    let stamp = Stamp {
        lamport: uint(lamport, "lamport")?,
        author: Principal::from_value(author)?,
        op_id: bytes_n(op_id, "op_id")?,
    };
    Ok(Won {
        position: Position::Seq(0),
        at_ms: 0,
        stamp,
    })
}

fn reg<T>(
    v: Value,
    what: &'static str,
    f: impl FnOnce(Value) -> Result<T, CodecError>,
) -> Result<Reg<T>, SnapshotError> {
    let [value, won] = array_n::<2>(v, what)?;
    Ok(Reg {
        value: f(value)?,
        won: read_stamp(won)?,
    })
}

fn read_entry(v: Value) -> Result<Entry, SnapshotError> {
    let mut map = MapReader::new(v, "entry")?;
    let id = InodeId(bytes_n(map.take(1)?, "id")?);
    let kind =
        EntryKind::from_code(uint(map.take(2)?, "kind")?).ok_or(CodecError::OutOfRange("kind"))?;
    let [by, at] = array_n::<2>(map.take(3)?, "created")?;
    let created = Stamped {
        by: Principal::from_value(by)?,
        at_ms: uint(at, "at_ms")?,
    };
    let [parent, name, won] = array_n::<3>(map.take(4)?, "place")?;
    let name = Name::new(&text(name, "name")?).map_err(|_| SnapshotError::Name)?;
    let place = Reg {
        value: Place {
            parent: InodeId(bytes_n(parent, "parent")?),
            name,
        },
        won: read_stamp(won)?,
    };
    let pointee = reg(map.take(5)?, "pointee", Pointee::from_value)?;
    pointee.value.check()?;
    let owner = reg(map.take(6)?, "owner", Principal::from_value)?;
    let mode = reg(map.take(7)?, "mode", Mode::from_value)?;
    let removal = match <[Value; 3]>::try_from(array(map.take(8)?, "removal")?) {
        Ok([by, at, won]) => Some(Reg {
            value: Some(Stamped {
                by: Principal::from_value(by)?,
                at_ms: uint(at, "at_ms")?,
            }),
            won: read_stamp(won)?,
        }),
        Err(empty) if empty.is_empty() => None,
        Err(_) => return Err(CodecError::OutOfRange("removal").into()),
    };
    map.finish()?;
    Ok(Entry {
        id,
        kind,
        created,
        place,
        pointee,
        owner,
        mode,
        removal,
        admissions: BTreeMap::new(),
    })
}

/// The tree a snapshot states, and its `upto_seq`. Its root is the volume's own (`vfs-model.md` §4).
pub fn decode(raw: &[u8]) -> Result<(Tree, u64), SnapshotError> {
    let mut map = MapReader::new(cbor_decode(raw)?, "snapshot")?;
    if uint(map.take(1)?, "v")? != SNAPSHOT_VERSION {
        return Err(SnapshotError::UpdateNeeded);
    }
    let volume =
        VolumeId::parse(&text(map.take(2)?, "volume")?).map_err(|_| SnapshotError::Volume)?;
    let upto_seq = uint(map.take(3)?, "upto_seq")?;
    let entries = array(map.take(4)?, "entries")?
        .into_iter()
        .map(read_entry)
        .collect::<Result<Vec<_>, _>>()?;
    let purged: Vec<InodeId> = array(map.take(5)?, "purged")?
        .into_iter()
        .map(|v| bytes_n(v, "purged").map(InodeId))
        .collect::<Result<_, _>>()?;
    map.finish()?;
    let sorted = |ids: &[InodeId]| ids.windows(2).all(|w| w[0] < w[1]);
    let ids: Vec<InodeId> = entries.iter().map(|e| e.id).collect();
    if !sorted(&ids) || !sorted(&purged) {
        return Err(CodecError::Unsorted.into());
    }
    let base = Tree::for_volume(volume.clone());
    let tree = Tree::from_parts(
        volume,
        base.root_owner,
        base.root_mode,
        entries,
        purged.into_iter().collect::<BTreeSet<_>>(),
    )
    .map_err(SnapshotError::Tree)?;
    Ok((tree, upto_seq))
}

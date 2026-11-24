//! Fixture JSON to `Value`, `Principal`, `Body`, `Op`.

use super::{hex, J};
use citadel_vfs::cbor::Value;
use citadel_vfs::model::{EntryKind, Mode, Pointee};
use citadel_vfs::op::{op_hash, Body, Header, Op, OpKind};
use citadel_vfs::{InodeId, Principal, VolumeId};

/// The fixtures' CBOR model: `{"$hex"}` bytes, `{"$u64"}` big integers, `{"$map"}` maps.
pub fn value(j: &J) -> Value {
    match j {
        J::Number(n) => Value::Uint(n.as_u64().unwrap_or_else(|| panic!("not a uint: {n}"))),
        J::String(s) => Value::text(s),
        J::Array(items) => Value::Array(items.iter().map(value).collect()),
        J::Object(o) => {
            if let Some(J::String(h)) = o.get("$hex") {
                return Value::Bytes(super::hex_vec(h));
            }
            if let Some(J::String(n)) = o.get("$u64") {
                return Value::Uint(n.parse().unwrap_or_else(|_| panic!("bad $u64 {n}")));
            }
            if let Some(J::Array(pairs)) = o.get("$map") {
                let pairs = pairs
                    .iter()
                    .map(|p| {
                        (
                            p[0].as_u64().unwrap_or_else(|| panic!("map key")),
                            value(&p[1]),
                        )
                    })
                    .collect();
                return Value::Map(pairs);
            }
            panic!("unknown model object {o:?}")
        }
        other => panic!("not in the model: {other}"),
    }
}

pub fn principal(j: &J) -> Principal {
    Principal::from_value(value(j)).unwrap_or_else(|e| panic!("principal {j}: {e:?}"))
}

pub fn id(j: &J, root: InodeId) -> InodeId {
    match j.as_str() {
        Some("root") => root,
        Some(h) => InodeId(hex::<16>(h)),
        None => panic!("not an id: {j}"),
    }
}

pub fn kind(name: &str) -> OpKind {
    let code = match name {
        "create" => 1,
        "place" => 2,
        "set_pointee" => 3,
        "remove" => 4,
        "restore" => 5,
        "purge" => 6,
        "chmod" => 7,
        "chown" => 8,
        "admit" => 9,
        "dismiss" => 10,
        "claim" => 11,
        other => panic!("unknown kind {other}"),
    };
    OpKind::from_code(code).unwrap_or_else(|| panic!("no kind {code}"))
}

fn name(j: &J) -> citadel_vfs::name::Name {
    let s = j.as_str().unwrap_or_else(|| panic!("name {j}"));
    citadel_vfs::name::Name::new(s).unwrap_or_else(|e| panic!("name {s}: {e:?}"))
}

fn mode(j: &J) -> Mode {
    let bits = j.as_u64().and_then(|m| u16::try_from(m).ok());
    bits.and_then(Mode::new)
        .unwrap_or_else(|| panic!("mode {j}"))
}

pub fn pointee(j: &J) -> Pointee {
    Pointee::from_value(value(j)).unwrap_or_else(|e| panic!("pointee {j}: {e:?}"))
}

pub fn body(kind: &str, b: &J, root: InodeId) -> Body {
    match kind {
        "create" => Body::Create {
            kind: match b["kind"].as_str() {
                Some("dir") => EntryKind::Dir,
                Some("file") => EntryKind::File,
                other => panic!("entry kind {other:?}"),
            },
            parent: id(&b["parent"], root),
            name: name(&b["name"]),
            pointee: pointee(&b["pointee"]),
            mode: mode(&b["mode"]),
            owner: principal(&b["owner"]),
        },
        "place" => Body::Place {
            parent: id(&b["parent"], root),
            name: name(&b["name"]),
        },
        "set_pointee" => Body::SetPointee {
            pointee: pointee(&b["pointee"]),
        },
        "remove" => Body::Remove,
        "restore" => Body::Restore,
        "purge" => Body::Purge,
        "chmod" => Body::Chmod {
            mode: mode(&b["mode"]),
        },
        "chown" => Body::Chown {
            owner: principal(&b["owner"]),
        },
        other => panic!("fixture body of kind {other} is not modelled here"),
    }
}

/// An op as a fixture lists it; `op_hash` is the header's hash unless the fixture gives one.
pub fn op(o: &J, volume: &VolumeId, root: InodeId) -> Op {
    let kind_name = o["kind"].as_str().unwrap_or_else(|| panic!("op kind"));
    let body = body(kind_name, &o["body"], root);
    let header = Header {
        volume: volume.to_string(),
        op_id: hex::<16>(o["op_id"].as_str().unwrap_or_else(|| panic!("op_id"))),
        author: principal(&o["author"]),
        lamport: o["lamport"].as_u64().unwrap_or_else(|| panic!("lamport")),
        prev: o.get("prev").and_then(J::as_str).map(hex::<32>),
        kind: kind(kind_name),
        target: id(&o["target"], root),
        body_hash: citadel_vfs::sha3_256(&[&body.encode()]),
        at_ms: o["at_ms"].as_u64().unwrap_or_else(|| panic!("at_ms")),
    };
    let op_hash = match o.get("op_hash").and_then(J::as_str) {
        Some(h) => hex::<32>(h),
        None => op_hash(&header.encode()),
    };
    Op {
        header,
        body: Some(body),
        op_hash,
    }
}

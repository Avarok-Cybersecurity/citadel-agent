//! The trees of `authorize/cases.json`, built from their parts.

use super::build::{admitted, id, pointee, principal};
use super::{hex, J};
use citadel_vfs::model::{Caps, EntryKind, Mode, Stamped};
use citadel_vfs::name::Name;
use citadel_vfs::op::Stamp;
use citadel_vfs::reduce::{Admission, Entry, Place, Position, Reg, Tree, Usage, Won};
use citadel_vfs::{InodeId, VolumeId};
use std::collections::{BTreeMap, BTreeSet};

fn reg<T>(value: T) -> Reg<T> {
    let stamp = Stamp {
        lamport: 1,
        author: citadel_vfs::Principal::Server,
        op_id: [0; 16],
    };
    Reg {
        value,
        won: Won {
            position: Position::Seq(0),
            stamp,
            at_ms: 0,
        },
    }
}

fn admission(a: &J) -> Admission {
    let cap = |i: usize| a["caps"][i].as_u64().unwrap_or_else(|| panic!("caps"));
    Admission {
        who: admitted(&a["who"]),
        by: principal(&a["by"]),
        expires_ms: a["expires_ms"]
            .as_u64()
            .unwrap_or_else(|| panic!("expires_ms")),
        caps: Caps {
            max_bytes: cap(0),
            max_entries: cap(1),
            max_ops_per_min: cap(2),
        },
        claims: a["claims"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(J::as_str)
            .map(String::from)
            .collect(),
        handed_over: BTreeSet::new(),
        used: Usage {
            bytes: a["used"]["bytes"].as_u64().unwrap_or(0),
            entries: a["used"]["entries"].as_u64().unwrap_or(0),
            recent_ms: Vec::new(),
        },
    }
}

pub fn tree(t: &J) -> Tree {
    let volume = VolumeId::parse(t["volume"].as_str().unwrap_or_default())
        .unwrap_or_else(|_| panic!("volume"));
    let root = InodeId::root(&volume);
    let entries = t["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            let id_ = id(&e["id"], root);
            let admissions: BTreeMap<_, _> = e["admissions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|a| {
                    let a = admission(a);
                    (a.who.key(), reg(Some(a)))
                })
                .collect();
            let removal = match &e["removal"] {
                J::Null => None,
                r => Some(reg(Some(Stamped {
                    by: principal(&r["by"]),
                    at_ms: r["at_ms"].as_u64().unwrap_or(0),
                }))),
            };
            Entry {
                id: id_,
                kind: if e["kind"] == "dir" {
                    EntryKind::Dir
                } else {
                    EntryKind::File
                },
                created: Stamped {
                    by: principal(&e["owner"]),
                    at_ms: 0,
                },
                place: reg(Place {
                    parent: id(&e["parent"], root),
                    name: Name::new(&format!("e{}", id_.to_hex()))
                        .unwrap_or_else(|e| panic!("{e:?}")),
                }),
                pointee: reg(pointee(&e["pointee"])),
                owner: reg(principal(&e["owner"])),
                mode: reg(Mode::new(e["mode"].as_u64().unwrap_or(0) as u16)
                    .unwrap_or_else(|| panic!("mode"))),
                removal,
                admissions,
            }
        })
        .collect();
    let root_mode = Mode::new(t["root"]["mode"].as_u64().unwrap_or(0) as u16)
        .unwrap_or_else(|| panic!("root mode"));
    Tree::from_parts(
        volume,
        principal(&t["root"]["owner"]),
        root_mode,
        entries,
        BTreeSet::new(),
    )
    .unwrap_or_else(|e| panic!("tree: {e:?}"))
}

pub fn link_id(j: &J) -> [u8; 16] {
    hex::<16>(j["$hex"].as_str().unwrap_or_default())
}

//! Folding a reduce fixture the way its volume folds.

use super::build::{op, principal};
use super::J;
use citadel_vfs::authorize::{RoleMask, VolumeCtx};
use citadel_vfs::limits::PEER_RETENTION_MS;
use citadel_vfs::op::Op;
use citadel_vfs::reduce::{
    GroupEvent, GroupEventKind, GroupFold, MergeFold, OrderedFold, SkipReason, Tree,
};
use citadel_vfs::{InodeId, VolumeId, VolumeMode};

pub struct Fixture {
    pub name: String,
    pub volume: VolumeId,
    pub ops: Vec<Op>,
    pub json: J,
}

pub fn load(text: &str) -> Fixture {
    let json = super::parse(text);
    let name = json["name"].as_str().unwrap_or_default().to_string();
    let volume = VolumeId::parse(json["volume"].as_str().unwrap_or_default())
        .unwrap_or_else(|_| panic!("{name}: volume"));
    let root = InodeId::root(&volume);
    assert_eq!(
        root.to_hex(),
        json["root"].as_str().unwrap_or_default(),
        "{name}: root id"
    );
    let ops = json["ops"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|o| op(o, &volume, root))
        .collect();
    Fixture {
        name,
        volume,
        ops,
        json,
    }
}

fn members(json: &J) -> Vec<u64> {
    json["members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(J::as_u64)
        .collect()
}

/// A fixture's tree, folded as its volume folds, ops in the order given.
pub fn fold(f: &Fixture, ops: &[Op]) -> Tree {
    if let Some(retention) = f.json.get("retention_ms").and_then(J::as_u64) {
        assert_eq!(
            retention, PEER_RETENTION_MS,
            "{}: retention is the peer constant",
            f.name
        );
    }
    if f.volume.mode() == VolumeMode::Merge {
        let tree = MergeFold::fold(f.volume.clone(), ops.iter().cloned())
            .unwrap_or_else(|| panic!("merge"));
        let mut incremental = MergeFold::new(f.volume.clone()).unwrap_or_else(|| panic!("merge"));
        for op in ops {
            incremental.insert(op.clone());
        }
        assert_eq!(
            *incremental.tree(),
            tree,
            "{}: one at a time differs from one fold",
            f.name
        );
        return tree;
    }
    if f.volume.has_kernel() {
        return ordered(f, ops);
    }
    group(f, ops)
}

fn ordered(f: &Fixture, ops: &[Op]) -> Tree {
    let ids = members(&f.json);
    let is_member = |cid: u64| ids.contains(&cid);
    let role = |_: u64| RoleMask::peer(false);
    let mut fold = OrderedFold::new(f.volume.clone()).unwrap_or_else(|| panic!("ordered"));
    for (i, op) in ops.iter().enumerate() {
        let ctx = VolumeCtx {
            kind: f.volume.kind(),
            is_member: &is_member,
            role: &role,
            retention_ms: PEER_RETENTION_MS,
            now_ms: op.header.at_ms,
        };
        let seq = i as u64 + 1;
        let _ = fold
            .apply(seq, op, &ctx)
            .unwrap_or_else(|h| panic!("{}: halt {h:?}", f.name));
    }
    fold.into_tree()
}

fn group(f: &Fixture, ops: &[Op]) -> Tree {
    let owner = f.json["owner_at_start"]
        .as_u64()
        .unwrap_or_else(|| panic!("owner_at_start"));
    let mut fold = GroupFold::new(f.volume.clone(), members(&f.json), owner)
        .unwrap_or_else(|| panic!("group"));
    assert_eq!(
        fold.tree().root_owner,
        principal(&f.json["root_owner"]),
        "{}: root owner",
        f.name
    );
    assert_eq!(
        u64::from(fold.tree().root_mode.bits()),
        f.json["root_mode"].as_u64().unwrap_or(0),
        "{}",
        f.name
    );
    let mut events: Vec<GroupEvent> = f.json["events"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| GroupEvent {
            seq: e["seq"].as_u64().unwrap_or(0),
            kind: match e["kind"].as_str() {
                Some("joined") => GroupEventKind::Joined,
                Some("left") => GroupEventKind::Left,
                Some("owner") => GroupEventKind::Owner,
                other => panic!("event {other:?}"),
            },
            cid: e["cid"].as_u64().unwrap_or(0),
        })
        .collect();
    events.reverse();
    let placed = f.json["ops"].as_array().cloned().unwrap_or_default();
    for (op, at) in ops.iter().zip(placed.iter()) {
        let seq = at["seq"].as_u64().unwrap_or_else(|| panic!("seq"));
        let index = at["index"]
            .as_u64()
            .and_then(|i| u32::try_from(i).ok())
            .unwrap_or(0);
        while events.last().is_some_and(|e| e.seq < seq) {
            let e = events.pop().unwrap_or_else(|| unreachable!());
            fold.event(e).unwrap_or_else(|h| panic!("{h:?}"));
        }
        let carrier = at["carrier"].as_u64().unwrap_or_else(|| panic!("carrier"));
        if !fold.carrier_ok(&op.header.author, carrier) {
            fold.skip(seq, index, op, SkipReason::NotAuthor)
                .unwrap_or_else(|h| panic!("{h:?}"));
            continue;
        }
        let _ = fold
            .apply(seq, index, op)
            .unwrap_or_else(|h| panic!("{}: halt {h:?}", f.name));
    }
    fold.tree().clone()
}

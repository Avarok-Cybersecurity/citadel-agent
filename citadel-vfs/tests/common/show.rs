//! A tree as the fixtures' `expect` shows it.

use super::J;
use citadel_vfs::cbor::Value;
use citadel_vfs::model::{EntryKind, Stamped};
use citadel_vfs::reduce::{EntryState, Tree};
use citadel_vfs::Principal;
use serde_json::json;

pub fn value(v: &Value) -> J {
    match v {
        Value::Uint(n) if *n > (1u64 << 53) => json!({ "$u64": n.to_string() }),
        Value::Uint(n) => json!(n),
        Value::Bytes(b) => json!({ "$hex": citadel_vfs::to_hex(b) }),
        Value::Text(t) => json!(t),
        Value::Array(items) => J::Array(items.iter().map(value).collect()),
        Value::Map(pairs) => {
            json!({ "$map": pairs.iter().map(|(k, v)| json!([k, value(v)])).collect::<Vec<_>>() })
        }
    }
}

pub fn principal(p: &Principal) -> J {
    value(&p.to_value())
}

fn stamped(s: &Stamped) -> J {
    json!({ "by": principal(&s.by), "at_ms": s.at_ms })
}

pub fn entries(tree: &Tree) -> J {
    let mut out: Vec<J> = tree
        .entries()
        .map(|e| {
            let parent = e.place.value.parent;
            let state = match tree.state(&e.id) {
                Some(EntryState::Removed) => "removed",
                Some(EntryState::Revfs) => "revfs",
                Some(EntryState::Shared) => "shared",
                None => "none",
            };
            json!({
                "id": e.id.to_hex(),
                "kind": if e.kind == EntryKind::Dir { "dir" } else { "file" },
                "parent": if parent == tree.root { "root".to_string() } else { parent.to_hex() },
                "name": e.place.value.name.as_str(),
                "shown": tree.shown_name(&e.id),
                "pointee": value(&e.pointee.value.to_value()),
                "owner": principal(&e.owner.value),
                "mode": e.mode.value.bits(),
                "removal": e.removal().map(stamped),
                "state": state,
                "created": stamped(&e.created),
            })
        })
        .collect();
    out.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    J::Array(out)
}

pub fn purged(tree: &Tree) -> J {
    J::Array(tree.purged().iter().map(|id| json!(id.to_hex())).collect())
}

/// The skipped ops, sorted, with `op_hash` only where `expected` lists one for that op.
pub fn skipped(tree: &Tree, expected: &J) -> (J, J) {
    let with_hash: Vec<&str> = expected
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s.get("op_hash").is_some())
        .filter_map(|s| s["op_id"].as_str())
        .collect();
    let mut got: Vec<J> = tree
        .skipped()
        .iter()
        .map(|s| {
            let op_id = citadel_vfs::to_hex(&s.op_id);
            let reason = serde_json::to_value(&s.reason).unwrap_or_else(|e| panic!("reason: {e}"));
            if with_hash.contains(&op_id.as_str()) {
                json!({ "op_id": op_id, "op_hash": citadel_vfs::to_hex(&s.op_hash), "reason": reason })
            } else {
                json!({ "op_id": op_id, "reason": reason })
            }
        })
        .collect();
    let mut want: Vec<J> = expected.as_array().cloned().unwrap_or_default();
    let key = |j: &J| j.to_string();
    got.sort_by_key(key);
    want.sort_by_key(key);
    (J::Array(got), J::Array(want))
}

/// Compare a tree with a fixture's `expect`, naming the fixture on failure.
pub fn assert_matches(name: &str, tree: &Tree, expect: &J) {
    assert_eq!(entries(tree), expect["entries"], "{name}: entries");
    let mut want_purged: Vec<J> = expect["purged"].as_array().cloned().unwrap_or_default();
    want_purged.sort_by_key(|j| j.to_string());
    assert_eq!(purged(tree), J::Array(want_purged), "{name}: purged");
    let (got, want) = skipped(tree, &expect["skipped"]);
    assert_eq!(got, want, "{name}: skipped");
}

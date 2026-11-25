//! Every case of `fixtures/authorize/cases.json` against `authorize` and the read checks.

mod common;

use citadel_vfs::authorize::{authorize, may_list, may_read_bytes, may_see, RoleMask, VolumeCtx};
use citadel_vfs::op::{op_hash, Header, Op};
use citadel_vfs::reduce::Tree;
use common::build::{body, id, kind, principal};
use common::{trees, J};
use std::collections::BTreeMap;

fn role_of(t: &J, cid: u64) -> RoleMask {
    if t["group_owner"].as_u64() == Some(cid) {
        return RoleMask::peer(true);
    }
    match &t["roles"] {
        J::Null => RoleMask::peer(false),
        roles => {
            let r = &roles[cid.to_string()];
            let bit = |k: &str| r[k].as_bool().unwrap_or(false);
            RoleMask {
                view: bit("view"),
                upload: bit("upload"),
                download: bit("download"),
                manage_files: bit("manage_files"),
                share_links: bit("share_links"),
            }
        }
    }
}

fn case(c: &J, t: &J, tree: &Tree) -> J {
    let members: Vec<u64> = t["members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(J::as_u64)
        .collect();
    let is_member = |cid: u64| members.contains(&cid);
    let role = |cid: u64| role_of(t, cid);
    let at_ms = c["op"]["at_ms"].as_u64();
    let now_ms = c["now_ms"]
        .as_u64()
        .or(t["now_ms"].as_u64())
        .or(at_ms)
        .unwrap_or(0);
    let ctx = VolumeCtx {
        kind: tree.volume.kind(),
        is_member: &is_member,
        role: &role,
        retention_ms: t["retention_ms"]
            .as_u64()
            .unwrap_or_else(|| panic!("retention")),
        now_ms,
    };
    let result = if let Some(read) = c.get("read") {
        let actor = principal(&read["actor"]);
        let target = id(&read["target"], tree.root);
        match read["fn"].as_str() {
            Some("may_list") => may_list(tree, &ctx, &actor, target),
            Some("may_read_bytes") => may_read_bytes(tree, &ctx, &actor, target),
            Some("may_see") => may_see(tree, &ctx, &actor, target),
            other => panic!("read fn {other:?}"),
        }
    } else {
        let o = &c["op"];
        let kind_name = o["kind"].as_str().unwrap_or_default();
        let body = body(kind_name, &o["body"], tree.root);
        let header = Header {
            volume: tree.volume.to_string(),
            op_id: [0x0f; 16],
            author: principal(&o["author"]),
            lamport: 1,
            prev: None,
            kind: kind(kind_name),
            target: id(&o["target"], tree.root),
            body_hash: citadel_vfs::sha3_256(&[&body.encode()]),
            at_ms: at_ms.unwrap_or_else(|| panic!("at_ms")),
        };
        let op = Op {
            op_hash: op_hash(&header.encode()),
            header,
            body: Some(body),
        };
        authorize(tree, &ctx, &op)
    };
    match result {
        Ok(()) => J::from("ok"),
        Err(denied) => serde_json::to_value(denied).unwrap_or_else(|e| panic!("{e}")),
    }
}

both! {
    fn every_case() {
        let file = common::parse(include_str!("contracts/rel-1124/authorize/cases.json"));
        let built: BTreeMap<&str, Tree> = file["trees"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, t)| (k.as_str(), trees::tree(t)))
            .collect();
        let mut wrong = Vec::new();
        let cases = file["cases"].as_array().cloned().unwrap_or_default();
        assert_eq!(cases.len(), 80, "case count");
        for c in &cases {
            let name = c["tree"].as_str().unwrap_or_default();
            let got = case(c, &file["trees"][name], &built[name]);
            if got != c["expect"] {
                wrong.push(format!("{} ({}): want {} got {}", c["id"], c["cell"], c["expect"], got));
            }
        }
        assert!(wrong.is_empty(), "{} of {} cases differ:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
    }
}

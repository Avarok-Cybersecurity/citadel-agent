//! Chat documents and files into volumes (`vfs-migration.md` §2, §5, §6), the migration op id,
//! and `legacy_name`.

mod common;

use citadel_vfs::authorize::{RoleMask, VolumeCtx};
use citadel_vfs::migrate::{
    legacy_name, migrate_chat_index, migrate_node_chat, migration_op_id, AttachmentEntry,
    IndexRecord, ShareSource, TitleSource, TitleStamp,
};
use citadel_vfs::op::OpKind;
use citadel_vfs::reduce::OrderedFold;
use citadel_vfs::{Namespace, Principal, VolumeId};
use common::build::principal;
use common::legacy::{describe_all, stamp};
use common::{parse, J};

fn lines(j: &J) -> Vec<String> {
    j.as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(String::from))
        .collect()
}

both! {
    fn migrate_op_id_kat() {
        let f = parse(include_str!("contracts/rel-1124/codec/inode-kat.json"));
        let k = &f["migrate_op_id"];
        let volume = VolumeId::parse(k["volume"].as_str().unwrap_or_default()).unwrap_or_else(|_| panic!("volume"));
        let ns = Namespace::parse(k["namespace"].as_str().unwrap_or_default()).unwrap_or_else(|| panic!("ns"));
        let kind = OpKind::from_code(k["kind"].as_u64().unwrap_or(0)).unwrap_or_else(|| panic!("kind"));
        let id = migration_op_id(&volume, &principal(&k["author"]), ns.as_str(), k["key"].as_str().unwrap_or_default(), kind)
            .unwrap_or_else(|_| panic!("too long"));
        assert_eq!(citadel_vfs::to_hex(&id), k["op_id_hex"]);
    }

    fn chat_index_corpus() {
        let f = parse(include_str!("migrate/chat-index-1001.json"));
        let me = f["me"].as_u64().unwrap_or(0);
        let entries: Vec<IndexRecord> = f["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| IndexRecord {
                doc_id: e["doc_id"].as_str().unwrap_or_default().into(),
                peer_cid: e["peer_cid"].as_u64().unwrap_or(0),
                title: (!e["title"].is_null()).then(|| TitleStamp {
                    title: e["title"]["title"].as_str().unwrap_or_default().into(),
                    at: e["title"]["at"].as_u64().unwrap_or(0),
                    by: e["title"]["by"].as_u64().unwrap_or(0),
                }),
            })
            .collect();
        let out = migrate_chat_index(me, &entries);
        let want = f["expect"].as_object().cloned().unwrap_or_default();
        assert_eq!(out.len(), want.len());
        for (volume, m) in &out {
            assert!(m.skipped.is_empty(), "{volume}: {:?}", m.skipped);
            assert_eq!(describe_all(&m.ops, volume), lines(&want[&volume.to_string()]), "{volume}");
        }
    }

    fn node_chat_corpus_folds_as_server_ops() {
        let f = parse(include_str!("migrate/node-chat-n1.json"));
        let documents: Vec<(String, Option<TitleSource>)> = f["documents"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|d| {
                let source = (!d[1].is_null()).then(|| TitleSource {
                    content: d[1]["content"].as_str().unwrap_or_default().into(),
                    sender_cid: d[1]["sender_cid"].as_u64().unwrap_or(0),
                });
                (d[0].as_str().unwrap_or_default().into(), source)
            })
            .collect();
        let files: Vec<(String, AttachmentEntry, Option<ShareSource>)> = f["files"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| {
                let e = &x[1];
                let entry = AttachmentEntry {
                    size_bytes: e["size_bytes"].as_u64().unwrap_or(0),
                    uploaders: e["uploaders"].as_object().into_iter().flatten().map(|(k, v)| (k.clone(), v.as_u64().unwrap_or(0))).collect(),
                    messages: e["messages"].as_array().into_iter().flatten().filter_map(J::as_u64).collect(),
                    uploaded_at_ms: e["uploaded_at_ms"].as_u64().unwrap_or(0),
                };
                let share = (!x[2].is_null()).then(|| ShareSource {
                    name: x[2]["name"].as_str().unwrap_or_default().into(),
                    mime: x[2]["mime"].as_str().unwrap_or_default().into(),
                });
                (x[0].as_str().unwrap_or_default().into(), entry, share)
            })
            .collect();
        let node_id = f["node_id"].as_str().unwrap_or_default();
        let m = migrate_node_chat(node_id, &documents, &files);
        let volume = VolumeId::node(node_id).unwrap_or_else(|_| panic!("node"));
        assert_eq!(describe_all(&m.ops, &volume), lines(&f["expect"]["ops"]));
        let skipped: Vec<J> = m.skipped.iter().map(|s| serde_json::json!([s.what, format!("{:?}", s.why)])).collect();
        assert_eq!(J::Array(skipped), f["expect"]["skipped"]);
        let members = [1001u64, 1002, 1003];
        let is_member = |cid: u64| members.contains(&cid);
        let role = |_: u64| RoleMask::peer(false);
        let mut fold = OrderedFold::new(volume.clone()).unwrap_or_else(|| panic!("ordered"));
        for (i, op) in stamp(&m.ops, &volume, &Principal::Server).iter().enumerate() {
            let ctx = VolumeCtx { kind: volume.kind(), is_member: &is_member, role: &role, retention_ms: 0, now_ms: op.header.at_ms };
            let applied = fold.apply(i as u64 + 1, op, &ctx).unwrap_or_else(|h| panic!("{h:?}"));
            assert!(applied.is_ok(), "op {i}: {applied:?}");
        }
        assert_eq!(fold.tree().entries().count(), 5);
    }

    fn legacy_names() {
        let name = |raw: &str| legacy_name(raw, "fallback").map(|n| n.as_str().to_string());
        assert_eq!(name("a/b\u{7}c").as_deref(), Some("a-b-c"));
        assert_eq!(name("").as_deref(), Some("fallback"));
        assert_eq!(name("..").as_deref(), Some("fallback"));
        assert_eq!(name("x\u{378}").as_deref(), Some("fallback"));
        assert_eq!(name("re\u{301}sume\u{301}").as_deref(), Some("r\u{e9}sum\u{e9}"));
        let long = name(&"\u{e9}".repeat(300)).unwrap_or_default();
        assert_eq!(long.len(), 254, "cut at a code-point boundary within NAME_MAX_BYTES");
        assert_eq!(legacy_name("", "a/b"), None, "a fallback that is not a name gives none");
    }
}

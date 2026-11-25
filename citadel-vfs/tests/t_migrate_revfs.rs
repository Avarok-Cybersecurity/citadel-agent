//! RE-VFS trees into volumes (`vfs-migration.md` §3, §4, §8): the corpus, twice-run and
//! two-importer convergence, the badge of a hosted file, and sent records.

mod common;

use citadel_vfs::migrate::{
    migrate_peer_tree, migrate_server_tree, Migration, Received, SkipReason,
};
use citadel_vfs::reduce::{MergeFold, Tree};
use citadel_vfs::views::{badge, Badge};
use citadel_vfs::{InodeId, Namespace, Principal, VolumeId};
use common::legacy::{describe_all, node, stamp};
use common::{hex, parse, J};
use std::collections::BTreeMap;

fn received(j: &J) -> BTreeMap<String, Received> {
    j.as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| {
            let sha256 = hex::<32>(v["sha256"].as_str().unwrap_or_default());
            (
                k.clone(),
                Received {
                    sha256,
                    len: v["len"].as_u64().unwrap_or_default(),
                },
            )
        })
        .collect()
}

fn peer(text: &str) -> (VolumeId, u64, Migration, J) {
    let f = parse(text);
    let (me, other) = (
        f["me"].as_u64().unwrap_or(0),
        f["peer"].as_u64().unwrap_or(0),
    );
    let m = migrate_peer_tree(me, other, &node(&f["tree"]), &received(&f["received"]));
    (
        VolumeId::pair(me, other).unwrap_or_else(|| panic!("pair")),
        me,
        m,
        f,
    )
}

fn check(volume: &VolumeId, m: &Migration, f: &J) {
    let want: Vec<String> = f["expect"]["ops"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(String::from))
        .collect();
    assert_eq!(describe_all(&m.ops, volume), want, "{}", f["source"]);
    let skipped: Vec<J> = m
        .skipped
        .iter()
        .map(|s| serde_json::json!([s.what, format!("{:?}", s.why)]))
        .collect();
    let mut want_skipped = f["expect"]["skipped"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut skipped_sorted = skipped.clone();
    skipped_sorted.sort_by_key(J::to_string);
    want_skipped.sort_by_key(J::to_string);
    assert_eq!(skipped_sorted, want_skipped, "{}", f["source"]);
}

const ALICE: &str = include_str!("migrate/peer-tree-1001.json");
const BOB: &str = include_str!("migrate/peer-tree-1002.json");

fn fold(volume: &VolumeId, ops: Vec<citadel_vfs::op::Op>) -> Tree {
    MergeFold::fold(volume.clone(), ops).unwrap_or_else(|| panic!("merge"))
}

both! {
    fn peer_tree_corpus() {
        for text in [ALICE, BOB] {
            let (volume, _, m, f) = peer(text);
            check(&volume, &m, &f);
        }
    }

    fn server_tree_corpus() {
        let f = parse(include_str!("migrate/server-tree-1001.json"));
        let me = f["me"].as_u64().unwrap_or(0);
        let m = migrate_server_tree(me, &node(&f["tree"]));
        check(&VolumeId::Personal { cid: me }, &m, &f);
    }

    fn a_pass_run_twice_writes_nothing_new() {
        let (volume, me, m, _) = peer(ALICE);
        let once = stamp(&m.ops, &volume, &Principal::member(me));
        let tree = fold(&volume, once.clone());
        assert!(tree.skipped().is_empty(), "{:?}", tree.skipped());
        let twice = fold(&volume, once.iter().chain(once.iter()).cloned().collect());
        assert_eq!(twice, tree);
    }

    fn two_importers_make_one_tree() {
        let (volume, alice, a, _) = peer(ALICE);
        let (_, bob, b, _) = peer(BOB);
        let a_ops = stamp(&a.ops, &volume, &Principal::member(alice));
        let b_ops = stamp(&b.ops, &volume, &Principal::member(bob));
        let both = fold(&volume, a_ops.iter().chain(b_ops.iter()).cloned().collect());
        assert!(both.skipped().is_empty(), "{:?}", both.skipped());
        // Archive, Projects, Résumés, plan.md, theirs.bin, Transfers, photo.jpg, report.pdf.
        assert_eq!(both.entries().count(), 8, "one entry per derived id");
        let plan = InodeId::derive(&volume, Namespace::RevfsFile, "f-remote").unwrap_or_else(|_| panic!("id"));
        let pointee = &both.entry(&plan).unwrap_or_else(|| panic!("plan.md")).pointee.value;
        assert_eq!(badge(pointee, &Principal::member(alice)), Some(Badge::Owner));
        assert_eq!(badge(pointee, &Principal::member(bob)), Some(Badge::Host));
        assert_eq!(badge(pointee, &Principal::member(1003)), Some(Badge::Other));
    }

    fn a_hosted_file_appears_only_from_its_owner() {
        let (volume, alice, a, _) = peer(ALICE);
        let (_, bob, b, _) = peer(BOB);
        let theirs = InodeId::derive(&volume, Namespace::RevfsFile, "f-hosted").unwrap_or_else(|_| panic!("id"));
        assert!(a.skipped.iter().any(|s| s.why == SkipReason::StoredForPeer && s.what == "/Projects/theirs.bin"));
        let hosts = fold(&volume, stamp(&a.ops, &volume, &Principal::member(alice)));
        assert!(!hosts.exists(&theirs), "the host's pass writes no entry for a file it stores for the peer");
        let owners = fold(&volume, stamp(&b.ops, &volume, &Principal::member(bob)));
        let pointee = &owners.entry(&theirs).unwrap_or_else(|| panic!("the owner's pass writes it")).pointee.value;
        assert_eq!(badge(pointee, &Principal::member(alice)), Some(Badge::Host));
    }

    fn a_sent_record_appears_only_from_the_receiver() {
        let (volume, alice, a, _) = peer(ALICE);
        let (_, bob, b, _) = peer(BOB);
        let report = InodeId::derive(&volume, Namespace::Transfer, "t-sent").unwrap_or_else(|_| panic!("id"));
        let senders = fold(&volume, stamp(&a.ops, &volume, &Principal::member(alice)));
        assert!(!senders.exists(&report), "the sender's pass writes no entry for a sent file");
        let receivers = fold(&volume, stamp(&b.ops, &volume, &Principal::member(bob)));
        assert!(receivers.exists(&report), "the receiver's pass writes it");
    }
}

//! The derived views (`vfs-model.md` §8, `vfs-ui.md` §3, §7, `vfs-outsiders.md` §5) on the
//! authorize fixture's node tree, and the snapshot codec on the reduce fixtures' trees.

mod common;

use citadel_vfs::authorize::{RoleMask, VolumeCtx};
use citadel_vfs::model::AdmittedKey;
use citadel_vfs::reduce::Tree;
use citadel_vfs::views::{
    effective_access, names_seen_by, outsider_view, recycle_bin, shared_with_me,
};
use citadel_vfs::{snapshot, InodeId, Principal};
use common::{fold, parse, show, trees};

fn node_tree() -> Tree {
    let file = parse(include_str!("contracts/rel-1124/authorize/cases.json"));
    trees::tree(&file["trees"]["node"])
}

fn id(last: u8) -> InodeId {
    let mut b = [0u8; 16];
    b[15] = last;
    InodeId(b)
}

const MEMBERS: [u64; 5] = [1001, 1002, 1003, 1004, 1005];

both! {
    fn recycle_bin_lists_the_top_most_removed() {
        let tree = node_tree();
        let mut bin: Vec<_> = recycle_bin(&tree).into_iter().map(|b| (b.id, b.removal.by)).collect();
        bin.sort();
        assert_eq!(bin, vec![(id(0xf3), Principal::member(1003)), (id(0xf7), Principal::member(1002))]);
    }

    fn an_outsider_is_served_only_its_subtree_without_removed_entries() {
        let tree = node_tree();
        let view = outsider_view(&tree, &Principal::member(2001), id(0xd4), 10_000).unwrap_or_else(|| panic!("admitted"));
        let ids: Vec<InodeId> = view.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![id(0xd4), id(0xd5), id(0xf5), id(0xf6)]);
        for e in &view {
            assert!(e.id == id(0xd4) || tree.is_under(&e.id, &id(0xd4)), "{:?} is outside", e.id);
            assert!(e.removal().is_none());
        }
        assert!(view[0].admissions.keys().all(|k| *k == AdmittedKey::Member { cid: 2001 }), "only its own admission");
        assert_eq!(outsider_view(&tree, &Principal::member(2001), id(0xd1), 10_000), None);
        assert_eq!(outsider_view(&tree, &Principal::member(2002), id(0xd4), 10_000), None);
    }

    fn shared_with_me_has_one_item_per_live_admission() {
        let tree = node_tree();
        let items = shared_with_me([&tree], &|_| false, 2001, 10_000);
        assert_eq!(items.len(), 1);
        assert_eq!((items[0].entry, &items[0].by), (id(0xd4), &Principal::member(1002)));
        assert!(shared_with_me([&tree], &|_| true, 2001, 10_000).is_empty(), "not in a volume one is a member of");
    }

    fn names_are_seen_by_members_and_admitted_outsiders() {
        let tree = node_tree();
        let seen = names_seen_by(&tree, id(0xf5), 10_000);
        assert!(seen.every_member);
        assert!(seen.outsiders.contains(&(id(0xd4), AdmittedKey::Member { cid: 2001 })));
        assert!(names_seen_by(&tree, id(0xf1), 10_000).outsiders.is_empty());
        assert!(names_seen_by(&tree, id(0xf7), 10_000).outsiders.is_empty(), "a removed entry is not shown outside");
        assert!(names_seen_by(&tree, id(0xf5), 100_000).outsiders.iter().all(|(_, k)| matches!(k, AdmittedKey::Member { .. })), "the link expired");
    }

    fn effective_access_agrees_with_the_cases() {
        let tree = node_tree();
        let is_member = |cid: u64| MEMBERS.contains(&cid);
        let role = |_: u64| RoleMask::peer(false);
        let ctx = VolumeCtx { kind: tree.volume.kind(), is_member: &is_member, role: &role, retention_ms: 0, now_ms: 10_000 };
        let guest = effective_access(&tree, &ctx, &Principal::member(2001), id(0xf5));
        assert_eq!(guest.read, Ok(()), "O02");
        assert!(guest.chmod.is_err(), "an outsider never changes modes");
        let outside = effective_access(&tree, &ctx, &Principal::member(2001), id(0xf1));
        assert!(outside.see.is_err() && outside.read.is_err(), "O10");
        let owner = effective_access(&tree, &ctx, &Principal::member(1001), id(0xd1));
        assert_eq!((owner.list, owner.create_in, owner.chmod), (Ok(()), Ok(()), Ok(())));
    }

    fn a_snapshot_round_trips_and_is_canonical() {
        for text in [
            include_str!("contracts/rel-1124/reduce/08a-edits-to-a-removed-entry.json"),
            include_str!("contracts/rel-1124/reduce/10-purge-is-terminal.json"),
            include_str!("contracts/rel-1124/reduce/14-what-a-server-may-write.json"),
        ] {
            let f = fold::load(text);
            let tree = fold::fold(&f, &f.ops);
            let bytes = snapshot::encode(&tree, 7);
            let (back, upto) = snapshot::decode(&bytes).unwrap_or_else(|e| panic!("{}: {e:?}", f.name));
            assert_eq!(upto, 7);
            assert_eq!(show::entries(&back), show::entries(&tree), "{}", f.name);
            assert_eq!(back.purged(), tree.purged(), "{}", f.name);
            assert_eq!(snapshot::encode(&back, 7), bytes, "{}: one encoding", f.name);
            let mut trailing = bytes.clone();
            trailing.push(0);
            assert!(snapshot::decode(&trailing).is_err());
        }
    }
}

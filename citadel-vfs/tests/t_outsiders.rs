//! Outsiders through the folds (`vfs-outsiders.md` §2–§4, §9): claims and their limit, the
//! admitter owning what an outsider creates, caps, and dismissal by `lasts` and by position.

mod common;

use citadel_vfs::authorize::{Cap, Denied, RoleMask, VolumeCtx};
use citadel_vfs::model::{Admitted, AdmittedKey, Caps, ClaimWhat, EntryKind, Mode, Pointee};
use citadel_vfs::name::Name;
use citadel_vfs::op::{Body, Draft, Op};
use citadel_vfs::reduce::{Judge, MergeFold, OrderedFold, SkipReason, Tree};
use citadel_vfs::{sha3_256, InodeId, Principal, VolumeId};
use common::keys::Key;

const D: InodeId = InodeId([0xd1; 16]);
const F: InodeId = InodeId([0xf1; 16]);

fn op(
    volume: &VolumeId,
    author: &Principal,
    lamport: u64,
    target: InodeId,
    body: Body,
    prev: Option<&Op>,
) -> Op {
    let mut op_id = [0u8; 16];
    op_id.copy_from_slice(&sha3_256(&[&author.encode(), &lamport.to_be_bytes()])[..16]);
    Draft {
        volume: volume.clone(),
        op_id,
        author: author.clone(),
        lamport,
        prev: prev.map(|p| p.op_hash),
        target,
        at_ms: lamport * 1000,
        body,
    }
    .to_op()
}

fn create(
    kind: EntryKind,
    parent: InodeId,
    name: &str,
    len: u64,
    mode: u16,
    owner: &Principal,
) -> Body {
    let pointee = match kind {
        EntryKind::Dir => Pointee::None,
        EntryKind::File => Pointee::Standard {
            sha256: [7; 32],
            len,
            mime: String::new(),
        },
    };
    Body::Create {
        kind,
        parent,
        name: Name::new(name).unwrap_or_else(|e| panic!("{e:?}")),
        pointee,
        mode: Mode::new(mode).unwrap_or_else(|| panic!("mode")),
        owner: owner.clone(),
    }
}

fn caps(max_bytes: u64) -> Caps {
    Caps {
        max_bytes,
        max_entries: 5,
        max_ops_per_min: 60,
    }
}

fn reason(tree: &Tree, op: &Op) -> Option<SkipReason> {
    tree.skipped()
        .iter()
        .find(|s| s.op_hash == op.op_hash)
        .map(|s| s.reason.clone())
}

struct Pair {
    volume: VolumeId,
    alice: Principal,
    link_id: [u8; 16],
    g1: Principal,
    base: Vec<Op>,
}

/// Alice makes `D` (o+rwx) and a link for one guest; guest key 1 claims it.
fn pair(max_bytes: u64) -> (Pair, Key, Key) {
    let volume = VolumeId::pair(1001, 1002).unwrap_or_else(|| panic!("pair"));
    let alice = Principal::member(1001);
    let (link, g1_key) = (Key::from_byte(0x11), Key::from_byte(0x21));
    let mut link_id = [0u8; 16];
    link_id
        .copy_from_slice(&sha3_256(&[b"citadel-vfs-link-id-v1", &[0], link.pk.as_bytes()])[..16]);
    let g1 = Principal::LinkGuest {
        link_id,
        key_fp: g1_key.fp.clone(),
    };
    let who = Admitted::Link {
        link_id,
        link_pk: link.pk.as_bytes().to_vec(),
        max_claims: 1,
    };
    let mkdir = op(
        &volume,
        &alice,
        1,
        D,
        create(
            EntryKind::Dir,
            InodeId::root(&volume),
            "Shared",
            0,
            0o777,
            &alice,
        ),
        None,
    );
    let admit = op(
        &volume,
        &alice,
        2,
        D,
        Body::Admit {
            who,
            expires_ms: 86_400_000,
            caps: caps(max_bytes),
        },
        Some(&mkdir),
    );
    let link_sig = link.sign_claim(&volume.to_string(), &D.0, &g1_key);
    let what = ClaimWhat::Key {
        key_pk: g1_key.pk.as_bytes().to_vec(),
        link_sig,
    };
    let claim = op(&volume, &g1, 3, D, Body::Claim { link_id, what }, None);
    (
        Pair {
            volume,
            alice,
            link_id,
            g1,
            base: vec![mkdir, admit, claim],
        },
        link,
        g1_key,
    )
}

fn fold(p: &Pair, more: &[&Op]) -> Tree {
    MergeFold::fold(
        p.volume.clone(),
        p.base
            .iter()
            .cloned()
            .chain(more.iter().map(|o| (*o).clone())),
    )
    .unwrap_or_else(|| panic!("merge"))
}

both! {
    fn a_claim_past_max_claims_is_skipped() {
        let (p, link, _) = pair(1000);
        let g2_key = Key::from_byte(0x22);
        let g2 = Principal::LinkGuest { link_id: p.link_id, key_fp: g2_key.fp.clone() };
        let what = ClaimWhat::Key { key_pk: g2_key.pk.as_bytes().to_vec(), link_sig: link.sign_claim(&p.volume.to_string(), &D.0, &g2_key) };
        let second = op(&p.volume, &g2, 4, D, Body::Claim { link_id: p.link_id, what }, None);
        let tree = fold(&p, &[&second]);
        assert_eq!(reason(&tree, &p.base[2]), None, "the first claim applies");
        assert_eq!(reason(&tree, &second), Some(SkipReason::Denied(Denied::ClaimsExhausted)));
    }

    fn what_an_outsider_creates_is_owned_by_its_admitter() {
        let (p, _, _) = pair(1000);
        let mine = op(&p.volume, &p.g1, 5, F, create(EntryKind::File, D, "a.txt", 1, 0o666, &p.g1), Some(&p.base[2]));
        let theirs = op(&p.volume, &p.g1, 5, F, create(EntryKind::File, D, "a.txt", 1, 0o666, &p.alice), Some(&p.base[2]));
        assert_eq!(reason(&fold(&p, &[&mine]), &mine), Some(SkipReason::Denied(Denied::OwnerNotAdmitter)));
        let tree = fold(&p, &[&theirs]);
        assert_eq!(reason(&tree, &theirs), None);
        assert_eq!(tree.owner(&F), Some(&p.alice));
    }

    fn an_outsider_stays_within_its_caps() {
        let (p, _, _) = pair(10);
        let first = op(&p.volume, &p.g1, 5, F, create(EntryKind::File, D, "a.txt", 8, 0o666, &p.alice), Some(&p.base[2]));
        let again = op(&p.volume, &p.g1, 6, F, Body::SetPointee { pointee: Pointee::Standard { sha256: [8; 32], len: 8, mime: String::new() } }, Some(&first));
        let tree = fold(&p, &[&first, &again]);
        assert_eq!(reason(&tree, &first), None);
        assert_eq!(reason(&tree, &again), Some(SkipReason::Denied(Denied::AdmissionCap(Cap::Bytes))));
    }

    fn a_dismissed_guests_late_op_is_cut_by_lasts() {
        let (p, _, _) = pair(1000);
        let made = op(&p.volume, &p.g1, 5, F, create(EntryKind::File, D, "a.txt", 1, 0o666, &p.alice), Some(&p.base[2]));
        let dismiss = op(&p.volume, &p.alice, 10, D, Body::Dismiss { who: AdmittedKey::Link { link_id: p.link_id }, lasts: vec![(p.g1.clone(), Some(made.op_hash))] }, Some(&p.base[1]));
        let late = op(&p.volume, &p.g1, 6, F, Body::SetPointee { pointee: Pointee::Standard { sha256: [9; 32], len: 1, mime: String::new() } }, Some(&made));
        let tree = fold(&p, &[&made, &dismiss, &late]);
        assert_eq!(reason(&tree, &made), None, "accepted before the dismissal");
        assert_eq!(reason(&tree, &late), Some(SkipReason::Denied(Denied::NotAdmitted)), "stamped before the Dismiss, but not in lasts");
        let mut incremental = MergeFold::new(p.volume.clone()).unwrap_or_else(|| panic!("merge"));
        for o in p.base.iter().chain([&late, &dismiss, &made]) {
            incremental.insert(o.clone());
        }
        assert_eq!(*incremental.tree(), tree, "arrival order does not matter");
    }

    fn a_dismissed_member_is_refused_by_position_in_an_ordered_volume() {
        let volume = VolumeId::node("n1").unwrap_or_else(|_| panic!("node"));
        let owner = Principal::member(1001);
        let outsider = Principal::member(2001);
        let members = [1001u64];
        let is_member = |cid: u64| members.contains(&cid);
        let role = |_: u64| RoleMask::peer(false);
        let ctx = |now_ms| VolumeCtx { kind: volume.kind(), is_member: &is_member, role: &role, retention_ms: 0, now_ms };
        let who = Admitted::Member { cid: 2001, id_fp: "aaaabbbbccccdddd".into() };
        let ops = [
            op(&volume, &Principal::Server, 1, D, create(EntryKind::Dir, InodeId::root(&volume), "Shared", 0, 0o777, &owner), None),
            op(&volume, &owner, 2, D, Body::Admit { who, expires_ms: 0, caps: caps(1000) }, None),
            op(&volume, &owner, 3, D, Body::Dismiss { who: AdmittedKey::Member { cid: 2001 }, lasts: Vec::new() }, None),
        ];
        let probe = op(&volume, &outsider, 1, F, create(EntryKind::File, D, "a.txt", 1, 0o666, &owner), None);
        let mut fold = OrderedFold::new(volume.clone()).unwrap_or_else(|| panic!("ordered"));
        for (i, o) in ops.iter().enumerate() {
            let c = ctx(o.header.at_ms);
            let _ = fold.apply(i as u64 + 1, o, &c).unwrap_or_else(|h| panic!("{h:?}"));
            let judged = fold.tree().check(&probe, &Judge::Every(&ctx(o.header.at_ms)));
            match i {
                1 => assert_eq!(judged, Ok(()), "admitted at position 2"),
                2 => assert_eq!(judged, Err(SkipReason::Denied(Denied::NotAdmitted)), "dismissed at position 3"),
                _ => {}
            }
        }
    }
}

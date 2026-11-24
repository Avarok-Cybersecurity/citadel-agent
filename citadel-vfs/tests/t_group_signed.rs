//! A group's carried ops judged in `vfs-group.md` §2's order: structure, carrier, bound key,
//! signature, duplicate, chain; a key not yet held is a wait.

mod common;

use citadel_identity::MlDsa65Pk;
use citadel_vfs::model::{EntryKind, Mode, Pointee};
use citadel_vfs::name::Name;
use citadel_vfs::op::{Body, Draft};
use citadel_vfs::reduce::{
    Applied, FoldHalt, GroupFold, GroupVerifier, KeyAnswer, SkipReason, Verdict,
};
use citadel_vfs::{InodeId, Principal, VolumeId};
use common::keys::Key;

fn volume() -> VolumeId {
    VolumeId::parse("group:1001:42").unwrap_or_else(|_| panic!("group"))
}

fn mkdir(parent: InodeId, name: &str, owner: u64) -> Body {
    Body::Create {
        kind: EntryKind::Dir,
        parent,
        name: Name::new(name).unwrap_or_else(|e| panic!("{e:?}")),
        pointee: Pointee::None,
        mode: Mode::new(0o770).unwrap_or_else(|| panic!("mode")),
        owner: Principal::member(owner),
    }
}

fn draft(
    cid: u64,
    lamport: u64,
    op_id: u8,
    target: u8,
    prev: Option<[u8; 32]>,
    body: Body,
) -> Draft {
    Draft {
        volume: volume(),
        op_id: [op_id; 16],
        author: Principal::member(cid),
        lamport,
        prev,
        target: InodeId([target; 16]),
        at_ms: lamport * 1000,
        body,
    }
}

both! {
    fn checks_run_in_the_tables_order() {
        let (k1, k2, k3) = (Key::from_byte(1), Key::from_byte(2), Key::from_byte(3));
        let held: Vec<(String, MlDsa65Pk)> = [&k1, &k2, &k3].iter().map(|k| (k.fp.clone(), k.pk.clone())).collect();
        let keys = |fp: &str| match held.iter().find(|(f, _)| f == fp) {
            Some((_, pk)) => KeyAnswer::Have(pk.clone()),
            None => KeyAnswer::Missing,
        };
        let none = |_: &str| KeyAnswer::Missing;
        let fold = GroupFold::new(volume(), [1001, 1002], 1001).unwrap_or_else(|| panic!("group"));
        let mut v = GroupVerifier::new(fold);
        let root = InodeId::root(&volume());

        let first = draft(1001, 1, 1, 0xd1, None, mkdir(root, "Plans", 1001));
        let first_hash = first.to_op().op_hash;
        let raw = k1.sign_op(&first).encode();
        assert_eq!(v.op(1, 0, 1001, &raw, &keys), Verdict::Folded(Ok(Applied::Yes)));
        assert_eq!(v.op(1, 1, 1001, &raw, &keys), Verdict::Folded(Ok(Applied::AlreadyHeld)), "the same op again");

        let by_bob = draft(1002, 2, 2, 0xd2, None, mkdir(root, "Bob", 1002));
        let carried_by_alice = k2.sign_op(&by_bob).encode();
        assert_eq!(v.op(2, 0, 1001, &carried_by_alice, &keys), Verdict::Folded(Err(SkipReason::NotAuthor)));

        let rekeyed = draft(1001, 2, 3, 0xd3, Some(first_hash), mkdir(root, "Other", 1001));
        assert_eq!(v.op(3, 0, 1001, &k3.sign_op(&rekeyed).encode(), &keys), Verdict::Folded(Err(SkipReason::KeyChanged)));

        let mut forged = k2.sign_op(&by_bob);
        forged.sig[0] ^= 1;
        assert_eq!(v.op(4, 0, 1002, &forged.encode(), &keys), Verdict::Folded(Err(SkipReason::BadSignature)));

        let good_bob = k2.sign_op(&by_bob).encode();
        assert_eq!(v.op(5, 0, 1002, &good_bob, &none), Verdict::WaitForKey(k2.fp.clone()));
        assert_eq!(v.op(5, 0, 1002, &good_bob, &keys), Verdict::Folded(Ok(Applied::Yes)), "the wait is retried at the same position");

        let stale = draft(1002, 3, 6, 0xd4, None, mkdir(root, "Stale", 1002));
        assert_eq!(v.op(6, 0, 1002, &k2.sign_op(&stale).encode(), &keys), Verdict::Folded(Err(SkipReason::StalePrev)));

        let reused = draft(1001, 3, 1, 0xd5, Some(first_hash), mkdir(root, "Reused", 1001));
        assert_eq!(v.op(7, 0, 1001, &k1.sign_op(&reused).encode(), &keys), Verdict::Folded(Err(SkipReason::Duplicate)));

        assert_eq!(v.op(8, 0, 1001, &[0xa0], &keys), Verdict::Folded(Err(SkipReason::Invalid)));

        let mut newer = k1.sign_op(&draft(1001, 4, 9, 0xd6, None, mkdir(root, "New", 1001)));
        newer.header[2] = 2;
        assert_eq!(v.op(9, 0, 1001, &newer.encode(), &keys), Verdict::Halt(FoldHalt::UpdateNeeded));

        assert_eq!(v.op(1, 0, 1001, &raw, &keys), Verdict::Halt(FoldHalt::OutOfOrder));
        assert_eq!(v.tree().entries().count(), 2, "Plans and Bob");
    }
}

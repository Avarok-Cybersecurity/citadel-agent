//! Properties (native only): a Merge fold is one tree in any arrival order and one op at a
//! time; it is always a tree; the codec is a bijection on canonical bytes; `authorize` is total
//! and monotone in role bits.
#![cfg(not(target_arch = "wasm32"))]

mod common;

use citadel_vfs::authorize::{authorize, RoleMask, VolumeCtx};
use citadel_vfs::cbor::{decode, Value};
use citadel_vfs::model::{EntryKind, Mode, Pointee};
use citadel_vfs::name::Name;
use citadel_vfs::op::{Body, Draft, Header, Op};
use citadel_vfs::reduce::{MergeFold, Tree};
use citadel_vfs::{InodeId, Principal, VolumeId};
use proptest::prelude::*;

fn volume() -> VolumeId {
    VolumeId::pair(1001, 1002).unwrap_or_else(|| panic!("pair"))
}

fn id(n: u8) -> InodeId {
    if n == 0 {
        return InodeId::root(&volume());
    }
    InodeId([n; 16])
}

fn member() -> BoxedStrategy<Principal> {
    prop_oneof![Just(Principal::member(1001)), Just(Principal::member(1002))].boxed()
}

fn mode() -> BoxedStrategy<Mode> {
    prop_oneof![Just(0o770u16), Just(0o700), Just(0o777), Just(0o500)]
        .prop_map(|m| Mode::new(m).unwrap_or_else(|| panic!("mode")))
        .boxed()
}

fn name() -> BoxedStrategy<Name> {
    prop_oneof![
        Just("a"),
        Just("A"),
        Just("b.txt"),
        Just("B.TXT"),
        Just("c")
    ]
    .prop_map(|n| Name::new(n).unwrap_or_else(|e| panic!("{e:?}")))
    .boxed()
}

fn body() -> BoxedStrategy<Body> {
    let pointee = (any::<u8>(), 0u64..100).prop_map(|(b, len)| Pointee::Standard {
        sha256: [b; 32],
        len,
        mime: String::new(),
    });
    prop_oneof![
        3 => (any::<bool>(), 0u8..5, name(), mode(), member(), pointee.clone()).prop_map(|(dir, p, name, mode, owner, file)| Body::Create {
            kind: if dir { EntryKind::Dir } else { EntryKind::File },
            parent: id(p),
            name,
            pointee: if dir { Pointee::None } else { file },
            mode,
            owner,
        }),
        2 => (0u8..5, name()).prop_map(|(p, name)| Body::Place { parent: id(p), name }),
        1 => pointee.prop_map(|pointee| Body::SetPointee { pointee }),
        1 => Just(Body::Remove),
        1 => Just(Body::Restore),
        1 => Just(Body::Purge),
        1 => mode().prop_map(|mode| Body::Chmod { mode }),
    ]
    .boxed()
}

fn op() -> BoxedStrategy<Op> {
    (member(), 1u64..30, 1u8..5, body(), 0u8..40)
        .prop_map(|(author, lamport, target, body, op_id)| {
            Draft {
                volume: volume(),
                op_id: [op_id; 16],
                author,
                lamport,
                prev: None,
                target: id(target),
                at_ms: lamport * 1000,
                body,
            }
            .to_op()
        })
        .boxed()
}

fn ops() -> BoxedStrategy<Vec<Op>> {
    prop::collection::vec(op(), 0..24).boxed()
}

fn assert_tree(t: &Tree) {
    for e in t.entries() {
        let parent = e.place.value.parent;
        assert!(
            parent == t.root || t.is_dir(&parent),
            "{:?} hangs from nothing",
            e.id
        );
        assert!(t.children(&parent).any(|c| *c == e.id), "children index");
        assert!(t.ancestors(&e.id).len() <= t.entries().count(), "no cycle");
        assert!(!t.purged().contains(&e.id));
    }
}

fn value() -> BoxedStrategy<Value> {
    let leaf = prop_oneof![
        any::<u64>().prop_map(Value::Uint),
        prop::collection::vec(any::<u8>(), 0..40).prop_map(Value::Bytes),
        "[a-zé]{0,12}".prop_map(Value::Text),
    ];
    leaf.prop_recursive(4, 32, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::btree_map(0u64..1000, inner, 0..6)
                .prop_map(|m| Value::Map(m.into_iter().collect())),
        ]
    })
    .boxed()
}

proptest! {
    #![proptest_config(ProptestConfig::default())]

    #[test]
    fn any_arrival_order_gives_one_tree(set in ops(), seed in any::<u64>()) {
        let once = MergeFold::fold(volume(), set.clone()).unwrap_or_else(|| panic!("merge"));
        assert_tree(&once);
        let mut shuffled = set.clone();
        let n = shuffled.len();
        for i in (1..n).rev() {
            shuffled.swap(i, (seed.wrapping_mul(6364136223846793005).wrapping_add(i as u64) % (i as u64 + 1)) as usize);
        }
        let mut incremental = MergeFold::new(volume()).unwrap_or_else(|| panic!("merge"));
        for o in &shuffled {
            incremental.insert(o.clone());
            assert_tree(incremental.tree());
        }
        prop_assert_eq!(incremental.tree(), &once);
        prop_assert_eq!(&MergeFold::fold(volume(), shuffled).unwrap_or_else(|| panic!("merge")), &once);
    }

    #[test]
    fn canonical_bytes_round_trip(v in value()) {
        let bytes = v.encode();
        prop_assert_eq!(decode(&bytes), Ok(v));
    }

    #[test]
    fn nothing_non_canonical_decodes(v in value(), at in any::<usize>(), to in any::<u8>()) {
        let mut bytes = v.encode();
        let i = at % bytes.len();
        bytes[i] = to;
        if let Ok(back) = decode(&bytes) {
            prop_assert_eq!(back.encode(), bytes);
        }
    }

    #[test]
    fn headers_and_bodies_round_trip(o in op()) {
        let header = o.header.encode();
        let back = Header::decode(&header);
        prop_assert_eq!(back.as_ref(), Ok(&o.header));
        let body = o.body.as_ref().unwrap_or_else(|| panic!("body"));
        let back = Body::decode(o.header.kind, &body.encode());
        prop_assert_eq!(back.as_ref(), Ok(body));
    }

    #[test]
    fn authorize_is_total_and_monotone_in_roles(set in ops(), probe in op(), bits in any::<[bool; 5]>()) {
        let tree = MergeFold::fold(volume(), set).unwrap_or_else(|| panic!("merge"));
        let is_member = |cid: u64| cid == 1001 || cid == 1002;
        let mask = |on: [bool; 5]| RoleMask { view: on[0], upload: on[1], download: on[2], manage_files: on[3], share_links: on[4] };
        let fewer = |_: u64| mask(bits);
        let all = |_: u64| mask([true; 5]);
        let ctx = |role: &dyn Fn(u64) -> RoleMask| -> bool {
            let ctx = VolumeCtx { kind: citadel_vfs::VolumeKind::Node, is_member: &is_member, role, retention_ms: 0, now_ms: 0 };
            authorize(&tree, &ctx, &probe).is_ok()
        };
        if ctx(&fewer) {
            prop_assert!(ctx(&all), "more role bits refused what fewer allowed");
        }
    }
}

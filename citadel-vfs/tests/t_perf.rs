//! The reducer, `authorize`, names, the snapshot and signature checks at the peer caps
//! (`PEER_VOLUME_LIMITS`), against a time budget; each run prints what it measured. Native only
//! (no clock on wasm32-unknown-unknown). Budgets are for a release build; a debug build gets 25x.
#![cfg(not(target_arch = "wasm32"))]

mod common;

use citadel_vfs::authorize::{authorize, RoleMask, VolumeCtx};
use citadel_vfs::limits::PEER_VOLUME_LIMITS;
use citadel_vfs::model::{EntryKind, Mode, Pointee};
use citadel_vfs::name::Name;
use citadel_vfs::op::{Body, Draft, Op, SignedOp};
use citadel_vfs::reduce::{MergeFold, SkipReason};
use citadel_vfs::{snapshot, InodeId, Principal, VolumeId};
use common::keys::Key;
use std::time::{Duration, Instant};

fn budget(release_ms: u64) -> Duration {
    Duration::from_millis(if cfg!(debug_assertions) {
        release_ms * 25
    } else {
        release_ms
    })
}

fn timed<T>(what: &str, release_ms: u64, per: usize, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    let took = start.elapsed();
    println!(
        "perf {what}: {took:?} total, {:?} each over {per}",
        took / per.max(1) as u32
    );
    assert!(
        took <= budget(release_ms),
        "{what} took {took:?}, over {:?}",
        budget(release_ms)
    );
    out
}

fn id(dir: u32, file: u32) -> InodeId {
    let mut b = [0u8; 16];
    b[..4].copy_from_slice(&dir.to_be_bytes());
    b[4..8].copy_from_slice(&file.to_be_bytes());
    b[15] = 1;
    InodeId(b)
}

/// 16 directories at the root and 4095 files in each: `max_entries` exactly.
fn at_the_caps(volume: &VolumeId) -> Vec<Op> {
    let author = Principal::member(1001);
    let mode = |m| Mode::new(m).unwrap_or_else(|| panic!("mode"));
    let mut ops = Vec::new();
    let mut lamport = 0u64;
    for d in 0..16u32 {
        for f in 0..4096u32 {
            lamport += 1;
            let (kind, parent, pointee, m) = if f == 0 {
                (EntryKind::Dir, InodeId::root(volume), Pointee::None, 0o770)
            } else {
                (
                    EntryKind::File,
                    id(d, 0),
                    Pointee::Standard {
                        sha256: [1; 32],
                        len: 9,
                        mime: String::new(),
                    },
                    0o660,
                )
            };
            let name = Name::new(&format!("n{f}-{d}")).unwrap_or_else(|e| panic!("{e:?}"));
            let mut op_id = [0u8; 16];
            op_id[..8].copy_from_slice(&lamport.to_be_bytes());
            let body = Body::Create {
                kind,
                parent,
                name,
                pointee,
                mode: mode(m),
                owner: author.clone(),
            };
            ops.push(
                Draft {
                    volume: volume.clone(),
                    op_id,
                    author: author.clone(),
                    lamport,
                    prev: None,
                    target: id(d, f),
                    at_ms: lamport,
                    body,
                }
                .to_op(),
            );
        }
    }
    ops
}

#[test]
fn at_the_peer_caps() {
    let volume = VolumeId::pair(1001, 1002).unwrap_or_else(|| panic!("pair"));
    let ops = at_the_caps(&volume);
    let n = ops.len();
    assert_eq!(n as u32, PEER_VOLUME_LIMITS.max_entries);
    let mut fold = MergeFold::new(volume.clone()).unwrap_or_else(|| panic!("merge"));
    timed("incremental fold, max_entries ops", 500, n, || {
        for op in &ops {
            fold.insert(op.clone());
        }
    });
    assert_eq!(fold.tree().entries().count(), n);
    assert!(fold.tree().skipped().is_empty());
    let whole = timed("one fold of the whole set", 500, n, || {
        MergeFold::fold(volume.clone(), ops.clone())
    });
    assert_eq!(whole.as_ref(), Some(fold.tree()));

    let mut over = ops[1].clone();
    over.header.op_id = [0xff; 16];
    over.header.lamport = n as u64 + 1;
    over.header.target = id(99, 1);
    over.op_hash = citadel_vfs::op::op_hash(&over.header.encode());
    fold.insert(over.clone());
    assert_eq!(
        fold.tree().skipped().last().map(|s| &s.reason),
        Some(&SkipReason::Limit),
        "one past max_entries"
    );

    let tree = fold.tree();
    let is_member = |cid: u64| cid == 1001 || cid == 1002;
    let role = |_: u64| RoleMask::peer(false);
    let ctx = VolumeCtx {
        kind: volume.kind(),
        is_member: &is_member,
        role: &role,
        retention_ms: 0,
        now_ms: 0,
    };
    let probes: Vec<&Op> = ops.iter().step_by(7).collect();
    timed("authorize", 100, probes.len(), || {
        for op in &probes {
            let _ = authorize(tree, &ctx, op);
        }
    });
    let names = timed("shown names of a full directory", 50, 4095, || {
        tree.shown_names(&id(3, 0))
    });
    assert_eq!(names.len(), 4095);
    let bytes = timed("snapshot encode", 300, n, || {
        snapshot::encode(tree, n as u64)
    });
    let back = timed("snapshot decode", 600, n, || snapshot::decode(&bytes));
    assert_eq!(back.map(|(t, _)| t.entries().count()).ok(), Some(n));
    println!("perf snapshot at the caps: {} bytes", bytes.len());
}

#[test]
fn signature_check_per_op() {
    let volume = VolumeId::pair(1001, 1002).unwrap_or_else(|| panic!("pair"));
    let key = Key::from_byte(9);
    let ops = at_the_caps(&volume);
    let signed: Vec<SignedOp> = ops[..200]
        .iter()
        .map(|op| {
            let body = op.body.clone().unwrap_or_else(|| panic!("body"));
            let draft = Draft {
                volume: volume.clone(),
                op_id: op.header.op_id,
                author: op.header.author.clone(),
                lamport: op.header.lamport,
                prev: None,
                target: op.header.target,
                at_ms: op.header.at_ms,
                body,
            };
            key.sign_op(&draft)
        })
        .collect();
    timed("decode + ML-DSA-65 verify", 100, signed.len(), || {
        for s in &signed {
            let raw = s.encode();
            let back = SignedOp::decode(&raw).unwrap_or_else(|e| panic!("{e:?}"));
            assert!(back.verify(&key.pk).is_ok());
        }
    });
}

#[test]
fn a_chain_at_max_depth() {
    let volume = VolumeId::pair(1001, 1002).unwrap_or_else(|| panic!("pair"));
    let author = Principal::member(1001);
    let mut parent = InodeId::root(&volume);
    let mut ops = Vec::new();
    for depth in 1..=65u32 {
        let body = Body::Create {
            kind: EntryKind::Dir,
            parent,
            name: Name::new("d").unwrap_or_else(|e| panic!("{e:?}")),
            pointee: Pointee::None,
            mode: Mode::new(0o770).unwrap_or_else(|| panic!("mode")),
            owner: author.clone(),
        };
        let target = id(1000, depth);
        let mut op_id = [0u8; 16];
        op_id[..4].copy_from_slice(&depth.to_be_bytes());
        ops.push(
            Draft {
                volume: volume.clone(),
                op_id,
                author: author.clone(),
                lamport: depth.into(),
                prev: None,
                target,
                at_ms: 1,
                body,
            }
            .to_op(),
        );
        parent = target;
    }
    let tree = timed("a chain one past max_depth", 20, 65, || {
        MergeFold::fold(volume, ops)
    })
    .unwrap_or_else(|| panic!("merge"));
    assert_eq!(
        tree.entries().count(),
        usize::from(PEER_VOLUME_LIMITS.max_depth)
    );
    assert_eq!(tree.skipped().len(), 1);
}

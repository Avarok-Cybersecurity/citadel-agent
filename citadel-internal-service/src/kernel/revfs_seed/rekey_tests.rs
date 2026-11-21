//! The re-key against fakes: objects keep their contents, and a pass that stops at ANY step
//! leaves every object readable under a key the account holds, then the next pass finishes.
//!
//! Control: in `rekey_one`, replace the `stage(...)` call with `Ok(())` (write the object over
//! its only readable copy) → the stop-at-any-step test and the second-regeneration test go red.

use super::fake::{Fake, Stored, World};
use super::*;
use crate::kernel::revfs_seed::index::{IndexEntry, Location, Stage};
use parking_lot::Mutex;

fn key(seed: u8) -> RevfsKey {
    RevfsKey::derive(&[seed; 32], "acme.example", 7).unwrap()
}

const OBJECTS: [(Location, &str); 3] = [
    (Location::Server, "/a.txt"),
    (Location::Server, "/dir/b.txt"),
    (Location::Peer(9), "/c.txt"),
];

/// Three objects sealed under root 1, indexed; the account's roots are now 2 (current) and 1.
fn world() -> (Fake, Keys) {
    let mut world = World::default();
    for (location, path) in OBJECTS {
        world.objects.insert(
            (location, path.to_string()),
            Stored::Sealed {
                key: key(1).id(),
                contents: path.as_bytes().to_vec(),
            },
        );
        world.index.upsert(IndexEntry {
            location,
            path: path.into(),
            level: 0,
            key_id: key(1).id(),
            stage: Stage::Settled,
        });
    }
    (
        Fake(Mutex::new(world)),
        Keys {
            current: key(2),
            retired: vec![key(1)],
        },
    )
}

async fn pass(fake: &Fake, keys: &Keys) -> Report {
    let entries = pending(&fake.0.lock().index, keys.current.id());
    run(fake, keys, entries).await
}

/// Each object's contents open from where its index entry says (where a later pass looks),
/// under a key in `keys`.
fn assert_every_object_readable(fake: &Fake, keys: &Keys, when: &str) {
    let world = fake.0.lock();
    for (location, path) in OBJECTS {
        let entry = world.index.get(location, path).expect("indexed");
        let readable = |at: String, id: KeyId| {
            keys.find(id).is_some()
                && world.objects.get(&(location, at))
                    == Some(&Stored::Sealed {
                        key: id,
                        contents: path.as_bytes().to_vec(),
                    })
        };
        let ok = match entry.stage {
            Stage::Settled => readable(path.to_string(), entry.key_id),
            // The staged copy, or (once it was deleted) the object it was written over.
            Stage::Staged { key_id } => {
                readable(staging(path), key_id) || readable(path.to_string(), key_id)
            }
        };
        assert!(ok, "{when}: {path} has no readable copy ({entry:?})");
    }
    assert!(
        world.locals.is_empty(),
        "{when}: a plaintext copy was left behind"
    );
}

fn assert_finished(fake: &Fake, keys: &Keys) {
    let world = fake.0.lock();
    assert!(pending(&world.index, keys.current.id()).is_empty());
    for (location, path) in OBJECTS {
        let at = (location, path.to_string());
        assert_eq!(
            world.objects.get(&at),
            Some(&Stored::Sealed {
                key: keys.current.id(),
                contents: path.as_bytes().to_vec()
            })
        );
        assert!(
            !world.objects.contains_key(&(location, staging(path))),
            "a staged copy was left"
        );
    }
}

#[tokio::test]
async fn a_finished_pass_seals_every_object_under_the_new_key_with_its_contents() {
    let (fake, keys) = world();
    let report = pass(&fake, &keys).await;
    assert!(report.finished(), "{report:?}");
    assert_eq!(report.rekeyed, 3);
    assert_finished(&fake, &keys);
    assert!(fake.0.lock().index.referenced() == vec![keys.current.id()]);
}

#[tokio::test]
async fn a_pass_stopped_at_any_step_leaves_every_object_readable_and_the_next_finishes() {
    let (probe, keys) = world();
    pass(&probe, &keys).await;
    let total = probe.0.lock().calls;
    for stop in 0..total {
        let (fake, keys) = world();
        fake.0.lock().stop_at = Some(stop);
        let report = pass(&fake, &keys).await;
        assert!(!report.finished(), "stopped at {stop} yet finished");
        assert_every_object_readable(&fake, &keys, &format!("stopped at call {stop}"));
        fake.0.lock().stop_at = None;
        let resumed = pass(&fake, &keys).await;
        assert!(
            resumed.finished(),
            "stopped at {stop}, resumed: {resumed:?}"
        );
        assert_finished(&fake, &keys);
    }
}

#[tokio::test]
async fn an_unreachable_peers_objects_wait_under_their_key() {
    let (fake, keys) = world();
    fake.0.lock().offline.push(Location::Peer(9));
    let report = pass(&fake, &keys).await;
    assert_eq!(report.unreachable, vec![Location::Peer(9)]);
    assert_eq!(report.rekeyed, 2);
    assert_every_object_readable(&fake, &keys, "peer offline");
    let referenced = fake.0.lock().index.referenced();
    assert!(
        referenced.contains(&key(1).id()),
        "the old root is still needed"
    );
    fake.0.lock().offline.clear();
    assert!(pass(&fake, &keys).await.finished());
    assert_finished(&fake, &keys);
}

#[tokio::test]
async fn a_second_regeneration_mid_rekey_still_finishes_under_the_newest_key() {
    let (fake, keys) = world();
    fake.0.lock().stop_at = Some(2);
    pass(&fake, &keys).await;
    fake.0.lock().stop_at = None;
    let newer = Keys {
        current: key(3),
        retired: vec![key(2), key(1)],
    };
    assert_every_object_readable(&fake, &newer, "before the second pass");
    assert!(pass(&fake, &newer).await.finished());
    assert_finished(&fake, &newer);
}

#[tokio::test]
async fn an_object_under_a_key_no_root_gives_is_reported_not_touched() {
    let (fake, _) = world();
    let keys = Keys {
        current: key(2),
        retired: vec![],
    };
    let report = pass(&fake, &keys).await;
    assert_eq!(report.failed.len(), 3);
    assert!(report.failed.iter().all(|(_, _, why)| why == UNKNOWN_KEY));
    assert_eq!(fake.0.lock().calls, 0, "nothing was read or written");
}

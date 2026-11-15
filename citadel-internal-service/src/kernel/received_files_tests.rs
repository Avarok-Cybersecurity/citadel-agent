use super::*;
use crate::kernel::conversations::kv::MemoryKv;
use crate::kernel::requests::file::file_identity::identify;
use std::collections::HashSet;

fn identity() -> (tempfile::TempDir, FileIdentity) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("photo.png");
    std::fs::write(&file, b"png").unwrap();
    let identity = identify(&file).unwrap();
    (dir, identity)
}

const ALL_ON_DISK: &(dyn Fn(&Path) -> bool + Sync) = &|_| true;

#[tokio::test]
async fn a_received_file_is_found_again_from_the_account_store_alone() {
    // A new agent process has no memory of the reception; only the account's store does.
    let (_dir, id) = identity();
    let store = MemoryKv::default();
    let saved = Path::new("/home/a/.citadel/transfers/9/photo.png");
    record(&store, 9, saved, id.clone(), 100, ALL_ON_DISK)
        .await
        .unwrap();
    assert_eq!(lookup(&store, 9, saved).await.unwrap(), Some(id));
}

#[tokio::test]
async fn the_record_names_its_sender() {
    let (_dir, id) = identity();
    let store = MemoryKv::default();
    let saved = Path::new("/t/9/photo.png");
    record(&store, 9, saved, id, 100, ALL_ON_DISK)
        .await
        .unwrap();
    assert_eq!(
        lookup(&store, 10, saved).await.unwrap(),
        None,
        "another peer is not the sender"
    );
    assert_eq!(
        lookup(&store, 9, Path::new("/t/9/other.png"))
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn past_the_bound_only_the_oldest_files_no_longer_on_disk_are_dropped() {
    let (_dir, id) = identity();
    let store = MemoryKv::default();
    // Two of the files are gone from disk; the rest are still there.
    let gone: HashSet<PathBuf> = [PathBuf::from("/t/f1"), PathBuf::from("/t/f3")].into();
    let on_disk = move |p: &Path| !gone.contains(p);
    for i in 0..MAX_RECEIVED_RECORDS {
        record(
            &store,
            9,
            Path::new(&format!("/t/f{i}")),
            id.clone(),
            i as u64,
            &on_disk,
        )
        .await
        .unwrap();
    }
    record(&store, 9, Path::new("/t/new"), id.clone(), 9999, &on_disk)
        .await
        .unwrap();
    assert_eq!(
        lookup(&store, 9, Path::new("/t/f1")).await.unwrap(),
        None,
        "the oldest gone file goes"
    );
    assert!(
        lookup(&store, 9, Path::new("/t/f3"))
            .await
            .unwrap()
            .is_some(),
        "one over, one dropped"
    );
    assert!(
        lookup(&store, 9, Path::new("/t/f0"))
            .await
            .unwrap()
            .is_some(),
        "a file still on disk is kept"
    );
}

#[tokio::test]
async fn files_still_on_disk_are_never_dropped_for_the_bound() {
    let (_dir, id) = identity();
    let store = MemoryKv::default();
    for i in 0..=MAX_RECEIVED_RECORDS {
        record(
            &store,
            9,
            Path::new(&format!("/t/f{i}")),
            id.clone(),
            i as u64,
            ALL_ON_DISK,
        )
        .await
        .unwrap();
    }
    assert!(lookup(&store, 9, Path::new("/t/f0"))
        .await
        .unwrap()
        .is_some());
}

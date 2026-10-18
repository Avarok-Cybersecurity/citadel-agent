use super::*;
use crate::kernel::requests::file::file_identity::identify;

fn key(peer_cid: Option<u64>, path: &str) -> PulledKey {
    PulledKey {
        peer_cid,
        virtual_directory: PathBuf::from(path),
    }
}

/// A real file's identity: the type is only ever built from metadata.
fn identity() -> (tempfile::TempDir, FileIdentity) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.docx");
    std::fs::write(&file, b"docx").unwrap();
    let identity = identify(&file).unwrap();
    (dir, identity)
}

fn recorded(now: Instant) -> (PulledReads, tempfile::TempDir) {
    let (dir, identity) = identity();
    let mut reads = PulledReads::default();
    reads.record(
        key(Some(9), "/shared/a.docx"),
        PathBuf::from("/data/transfers/9/a.docx"),
        identity,
        now,
    );
    (reads, dir)
}

/// Begins and settles one successful read of `len` bytes at `offset`.
fn read_ok(reads: &mut PulledReads, k: &PulledKey, offset: u64, len: usize, now: Instant) {
    let ticket = reads.begin_read(k, offset, len as u32, now).unwrap();
    reads.settle(k, ticket.generation, Some((offset, len)));
}

#[test]
fn a_recorded_pull_is_readable_by_the_name_it_was_pulled_with() {
    let now = Instant::now();
    let (mut reads, _dir) = recorded(now);
    let ticket = reads
        .begin_read(
            &key(Some(9), "/shared/a.docx"),
            0,
            MAX_READ_CHUNK_BYTES,
            now,
        )
        .unwrap();
    assert_eq!(ticket.saved, PathBuf::from("/data/transfers/9/a.docx"));
}

#[test]
fn anything_not_pulled_is_refused_with_pull_it_first() {
    let now = Instant::now();
    let (mut reads, _dir) = recorded(now);
    for other in [
        key(Some(9), "/shared/b.docx"),
        key(Some(8), "/shared/a.docx"),
        key(None, "/shared/a.docx"),
    ] {
        let refused = reads.begin_read(&other, 0, 1, now).unwrap_err();
        assert!(refused.contains("pull it first"), "{refused}");
    }
}

#[test]
fn an_expired_pull_is_refused() {
    let then = Instant::now();
    let (mut reads, _dir) = recorded(then);
    let refused = reads
        .begin_read(
            &key(Some(9), "/shared/a.docx"),
            0,
            1,
            then + PICKED_FILE_TTL,
        )
        .unwrap_err();
    assert!(refused.contains("expired"), "{refused}");
}

#[test]
fn max_len_is_bounded_on_both_sides() {
    let now = Instant::now();
    let (mut reads, _dir) = recorded(now);
    let k = key(Some(9), "/shared/a.docx");
    assert!(reads
        .begin_read(&k, 0, MAX_READ_CHUNK_BYTES + 1, now)
        .is_err());
    assert!(reads.begin_read(&k, 0, 0, now).is_err());
    assert!(reads.begin_read(&k, 0, MAX_READ_CHUNK_BYTES, now).is_ok());
}

#[test]
fn reads_go_in_order_from_zero() {
    let now = Instant::now();
    let (mut reads, _dir) = recorded(now);
    let k = key(Some(9), "/shared/a.docx");
    assert!(
        reads.begin_read(&k, 5, 10, now).is_err(),
        "a read starts at 0"
    );
    read_ok(&mut reads, &k, 0, 10, now);
    for skipped in [9, 11, 20] {
        let refused = reads.begin_read(&k, skipped, 10, now).unwrap_err();
        assert!(refused.contains("out of order"), "{refused}");
    }
    read_ok(&mut reads, &k, 10, 10, now);
    read_ok(&mut reads, &k, 0, 10, now);
}

#[test]
fn one_read_is_in_flight_per_record() {
    let now = Instant::now();
    let (mut reads, _dir) = recorded(now);
    let k = key(Some(9), "/shared/a.docx");
    let first = reads.begin_read(&k, 0, 10, now).unwrap();
    let refused = reads.begin_read(&k, 0, 10, now).unwrap_err();
    assert!(refused.contains("in progress"), "{refused}");
    reads.settle(&k, first.generation, None);
    assert!(
        reads.begin_read(&k, 0, 10, now).is_ok(),
        "a failed read releases the record"
    );
}

#[test]
fn an_abandoned_read_releases_the_record_after_its_timeout() {
    let then = Instant::now();
    let (mut reads, _dir) = recorded(then);
    let k = key(Some(9), "/shared/a.docx");
    reads.begin_read(&k, 0, 10, then).unwrap();
    assert!(reads
        .begin_read(&k, 0, 10, then + READ_IN_FLIGHT_TIMEOUT)
        .is_ok());
}

#[test]
fn a_read_from_before_a_re_pull_settles_nothing() {
    let now = Instant::now();
    let (mut reads, dir) = recorded(now);
    let k = key(Some(9), "/shared/a.docx");
    let old = reads.begin_read(&k, 0, 10, now).unwrap();
    let fresh = identify(&dir.path().join("a.docx")).unwrap();
    reads.record(
        k.clone(),
        PathBuf::from("/data/transfers/9/a (1).docx"),
        fresh,
        now,
    );
    reads.settle(&k, old.generation, Some((0, 10)));
    let refused = reads.begin_read(&k, 10, 10, now).unwrap_err();
    assert!(
        refused.contains("starts at 0"),
        "the old file's progress must not carry over: {refused}"
    );
    let ticket = reads.begin_read(&k, 0, 10, now).unwrap();
    assert_ne!(ticket.generation, old.generation);
    assert_eq!(ticket.saved, PathBuf::from("/data/transfers/9/a (1).docx"));
}

#[test]
fn the_table_is_bounded() {
    let now = Instant::now();
    let (_dir, identity) = identity();
    let mut reads = PulledReads::default();
    for i in 0..(MAX_PICKED_FILES + 5) {
        let at = now + Duration::from_millis(i as u64);
        reads.record(
            key(None, &format!("/f{i}")),
            PathBuf::from("/x"),
            identity.clone(),
            at,
        );
    }
    assert_eq!(reads.entries.len(), MAX_PICKED_FILES);
    assert!(
        !reads.entries.contains_key(&key(None, "/f0")),
        "the oldest goes"
    );
}

#[test]
fn a_chunk_is_at_most_what_was_asked_and_what_is_left() {
    assert_eq!(plan_chunk(10, 0, 4), Ok(4));
    assert_eq!(plan_chunk(10, 8, 4), Ok(2));
    assert_eq!(
        plan_chunk(0, 0, 4),
        Ok(0),
        "an empty file is one empty chunk"
    );
}

#[test]
fn a_chunk_past_the_end_is_refused() {
    assert!(plan_chunk(10, 10, 4).is_err());
    assert!(plan_chunk(10, 11, 4).is_err());
    assert!(plan_chunk(0, 1, 4).is_err());
    assert!(plan_chunk(10, u64::MAX, u32::MAX).is_err());
}

#[test]
fn a_file_over_the_limit_is_refused_before_any_bytes() {
    assert!(plan_chunk(MAX_BROWSER_TRANSFER_TOTAL_BYTES + 1, 0, 1).is_err());
    assert_eq!(plan_chunk(MAX_BROWSER_TRANSFER_TOTAL_BYTES, 0, 1), Ok(1));
}

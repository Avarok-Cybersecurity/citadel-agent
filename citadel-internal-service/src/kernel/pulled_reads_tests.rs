use super::*;
use std::time::Duration;

fn key(peer_cid: Option<u64>, path: &str) -> PulledKey {
    PulledKey {
        peer_cid,
        virtual_directory: PathBuf::from(path),
    }
}

fn recorded(now: Instant) -> PulledReads {
    let mut reads = PulledReads::default();
    reads.record(
        key(Some(9), "/shared/a.docx"),
        PathBuf::from("/data/transfers/9/a.docx"),
        now,
    );
    reads
}

#[test]
fn a_recorded_pull_is_readable_by_the_name_it_was_pulled_with() {
    let now = Instant::now();
    let reads = recorded(now);
    assert_eq!(
        reads.begin_read(
            &key(Some(9), "/shared/a.docx"),
            0,
            MAX_READ_CHUNK_BYTES,
            now
        ),
        Ok(PathBuf::from("/data/transfers/9/a.docx"))
    );
}

#[test]
fn anything_not_pulled_is_refused_with_pull_it_first() {
    let now = Instant::now();
    let reads = recorded(now);
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
    let reads = recorded(then);
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
    let reads = recorded(now);
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
    let mut reads = recorded(now);
    let k = key(Some(9), "/shared/a.docx");
    assert!(
        reads.begin_read(&k, 5, 10, now).is_err(),
        "a read starts at 0"
    );
    reads.finish_read(&k, 0, 10);
    assert!(reads.begin_read(&k, 10, 10, now).is_ok());
    for skipped in [9, 11, 20] {
        let refused = reads.begin_read(&k, skipped, 10, now).unwrap_err();
        assert!(refused.contains("out of order"), "{refused}");
    }
    assert!(reads.begin_read(&k, 0, 10, now).is_ok(), "0 restarts");
}

#[test]
fn pulling_again_resets_the_read() {
    let now = Instant::now();
    let mut reads = recorded(now);
    let k = key(Some(9), "/shared/a.docx");
    reads.finish_read(&k, 0, 10);
    reads.record(
        k.clone(),
        PathBuf::from("/data/transfers/9/a (1).docx"),
        now,
    );
    assert!(reads.begin_read(&k, 10, 10, now).is_err());
    assert_eq!(
        reads.begin_read(&k, 0, 10, now),
        Ok(PathBuf::from("/data/transfers/9/a (1).docx"))
    );
}

#[test]
fn the_table_is_bounded() {
    let now = Instant::now();
    let mut reads = PulledReads::default();
    for i in 0..(MAX_PICKED_FILES + 5) {
        let at = now + Duration::from_millis(i as u64);
        reads.record(key(None, &format!("/f{i}")), PathBuf::from("/x"), at);
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

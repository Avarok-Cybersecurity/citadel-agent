use super::*;

fn upload(total: u64, received: u64, in_flight: &Arc<AtomicU64>) -> StagedUpload {
    in_flight.fetch_add(total - received, Ordering::SeqCst);
    StagedUpload {
        dir: PathBuf::from("/root/u"),
        path: PathBuf::from("/root/u/f.bin"),
        total,
        received,
        writing: false,
        started: Instant::now(),
        reservation: Reservation::new(in_flight.clone(), total - received),
    }
}

#[test]
fn the_first_chunk_starts_at_zero() {
    assert_eq!(plan_chunk(None, 0, 10, 30), ChunkPlan::Start);
    assert!(matches!(plan_chunk(None, 10, 10, 30), ChunkPlan::Refuse(_)));
}

#[test]
fn a_later_chunk_lands_only_where_the_last_one_ended() {
    let in_flight = Arc::new(AtomicU64::new(0));
    let u = upload(30, 10, &in_flight);
    assert_eq!(plan_chunk(Some(&u), 10, 10, 30), ChunkPlan::Append);
    assert!(matches!(
        plan_chunk(Some(&u), 0, 10, 30),
        ChunkPlan::Refuse(_)
    ));
    assert!(matches!(
        plan_chunk(Some(&u), 20, 10, 30),
        ChunkPlan::Refuse(_)
    ));
    assert!(matches!(
        plan_chunk(Some(&u), 10, 10, 31),
        ChunkPlan::Refuse(_)
    ));
}

#[test]
fn a_chunk_cannot_overrun_the_file_or_the_limits() {
    assert!(matches!(plan_chunk(None, 0, 31, 30), ChunkPlan::Refuse(_)));
    assert!(matches!(
        plan_chunk(None, 0, MAX_STAGE_CHUNK_BYTES + 1, u64::MAX),
        ChunkPlan::Refuse(_)
    ));
    assert!(matches!(
        plan_chunk(None, 0, 10, MAX_STAGED_UPLOAD_BYTES + 1),
        ChunkPlan::Refuse(_)
    ));
    assert!(matches!(plan_chunk(None, 0, 0, 30), ChunkPlan::Refuse(_)));
    assert!(matches!(plan_chunk(None, 0, 1, 0), ChunkPlan::Refuse(_)));
    assert_eq!(
        plan_chunk(None, 0, 10, MAX_STAGED_UPLOAD_BYTES),
        ChunkPlan::Start
    );
}

#[test]
fn a_chunk_is_refused_while_the_previous_one_is_written() {
    let in_flight = Arc::new(AtomicU64::new(0));
    let mut u = upload(30, 10, &in_flight);
    u.writing = true;
    assert!(matches!(
        plan_chunk(Some(&u), 10, 10, 30),
        ChunkPlan::Refuse(_)
    ));
}

#[test]
fn only_a_complete_upload_can_be_sent() {
    let in_flight = Arc::new(AtomicU64::new(0));
    let mut t = StagedUploads::default();
    let (part, done) = (Uuid::new_v4(), Uuid::new_v4());
    assert!(t.insert(part, upload(30, 10, &in_flight)).is_ok());
    assert!(t.insert(done, upload(30, 30, &in_flight)).is_ok());
    assert!(t.take_complete(&part).is_err());
    assert!(t.take_complete(&done).is_ok());
    assert!(t.take_complete(&done).is_err(), "taken once");
    assert!(t.take_complete(&Uuid::new_v4()).is_err());
}

#[test]
fn an_id_in_use_is_not_replaced() {
    let in_flight = Arc::new(AtomicU64::new(0));
    let mut t = StagedUploads::default();
    let id = Uuid::new_v4();
    assert!(t.insert(id, upload(30, 10, &in_flight)).is_ok());
    assert!(t.insert(id, upload(30, 0, &in_flight)).is_err());
    assert_eq!(t.get(&id).map(|u| u.received), Some(10));
}

#[test]
fn the_reservation_shrinks_as_bytes_land_and_returns_the_rest_on_drop() {
    let in_flight = Arc::new(AtomicU64::new(0));
    let mut u = upload(30, 0, &in_flight);
    u.reservation.landed(10);
    assert_eq!(in_flight.load(Ordering::SeqCst), 20);
    drop(u);
    assert_eq!(in_flight.load(Ordering::SeqCst), 0);
}

#[test]
fn a_stale_upload_is_pruned_and_its_bytes_returned() {
    let in_flight = Arc::new(AtomicU64::new(0));
    let mut t = StagedUploads::default();
    let id = Uuid::new_v4();
    assert!(t.insert(id, upload(30, 10, &in_flight)).is_ok());
    let later = Instant::now() + Duration::from_secs(601);
    assert_eq!(
        t.prune(later, Duration::from_secs(600)),
        vec![PathBuf::from("/root/u")]
    );
    assert!(t.get(&id).is_none());
    assert_eq!(in_flight.load(Ordering::SeqCst), 0);
}

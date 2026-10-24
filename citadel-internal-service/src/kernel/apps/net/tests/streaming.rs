//! A streamed body moves only against credit, and ends on cancel or a bad grant.

use super::super::stream::{Control, StreamIds, StreamSink, Streams};
use super::*;
use citadel_ihttp_types::{INITIAL_CREDIT, MAX_BODY_CHUNK_BYTES};
use citadel_internal_service_types::{
    AppFetchLimit, AppFetchRefusal, AppFetchUnavailable, InternalServiceResponse,
};
use parking_lot::Mutex;
use uuid::Uuid;

#[derive(Default)]
struct Recorded(Mutex<Vec<InternalServiceResponse>>);

impl StreamSink for Recorded {
    fn emit(&self, response: InternalServiceResponse) -> bool {
        self.0.lock().push(response);
        true
    }
}

impl Recorded {
    fn chunks(&self) -> usize {
        self.0
            .lock()
            .iter()
            .filter(|r| matches!(r, InternalServiceResponse::AppFetchStreamChunk(_)))
            .count()
    }
    fn last(&self) -> Option<InternalServiceResponse> {
        self.0.lock().last().cloned()
    }
}

struct Running {
    sink: Arc<Recorded>,
    streams: Arc<Streams>,
    connection: Uuid,
    fetch_id: Uuid,
    task: tokio::task::JoinHandle<()>,
}

/// Open a stream over `chunks` and run it in the background.
async fn start(chunks: Vec<Vec<u8>>) -> Running {
    let w = world();
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    w.connector.reply(Reply {
        status: 200,
        headers: vec![],
        chunks,
    });
    let (connection, fetch_id) = (Uuid::new_v4(), Uuid::new_v4());
    let streams = Arc::new(Streams::default());
    let mut controls = streams.register(connection, fetch_id).expect("first");
    assert!(
        streams.register(connection, fetch_id).is_none(),
        "a duplicate id was registered"
    );
    let request = get("https://api.acme.com/feed");
    let only = grant(false, &["api.acme.com"]);
    let install = key();
    let input = w.input(&install, APP, &only, &request);
    let opened = w.net.open(input, deadline()).await.expect("allowed");
    let sink = Arc::new(Recorded::default());
    let (net, task_sink) = (w.net.clone(), sink.clone());
    let task = tokio::spawn(async move {
        let ids = StreamIds {
            cid: 42,
            fetch_id,
            key: key(),
        };
        net.stream(opened, &ids, &mut controls, task_sink.as_ref(), deadline())
            .await;
    });
    Running {
        sink,
        streams,
        connection,
        fetch_id,
        task,
    }
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn chunks_stop_at_the_credit_and_resume_when_more_is_granted() {
    let run = start((0..10).map(|n| vec![n; 10]).collect()).await;
    settle().await;
    assert_eq!(
        run.sink.chunks(),
        INITIAL_CREDIT as usize,
        "sent past its credit"
    );
    assert!(run
        .streams
        .control(run.connection, run.fetch_id, Control::Credit(2)));
    settle().await;
    assert_eq!(run.sink.chunks(), INITIAL_CREDIT as usize + 2);
    // Another connection cannot grant this stream credit.
    assert!(!run
        .streams
        .control(Uuid::new_v4(), run.fetch_id, Control::Credit(64)));
    run.streams
        .control(run.connection, run.fetch_id, Control::Credit(64));
    run.task.await.expect("ran");
    assert_eq!(run.sink.chunks(), 10);
    match run.sink.last() {
        Some(InternalServiceResponse::AppFetchStreamEnd(end)) => assert_eq!(end.body_bytes, 100),
        other => panic!("not ended: {other:?}"),
    }
    assert!(matches!(
        run.sink.0.lock().first(),
        Some(InternalServiceResponse::AppFetchStreamHead(_))
    ));
}

#[tokio::test]
async fn a_large_upstream_chunk_is_split_to_the_chunk_size() {
    let run = start(vec![vec![7; MAX_BODY_CHUNK_BYTES * 2 + 5]]).await;
    run.task.await.expect("ran");
    let sizes: Vec<usize> = run
        .sink
        .0
        .lock()
        .iter()
        .filter_map(|r| match r {
            InternalServiceResponse::AppFetchStreamChunk(c) => Some(c.bytes.len()),
            _ => None,
        })
        .collect();
    assert_eq!(sizes, vec![MAX_BODY_CHUNK_BYTES, MAX_BODY_CHUNK_BYTES, 5]);
}

#[tokio::test]
async fn cancel_and_a_bad_credit_grant_each_end_the_stream() {
    for (control, refusal) in [
        (
            Control::Cancel,
            AppFetchRefusal::Unavailable(AppFetchUnavailable::Cancelled),
        ),
        (
            Control::Credit(0),
            AppFetchRefusal::Limit(AppFetchLimit::Credit),
        ),
        (
            Control::Credit(65),
            AppFetchRefusal::Limit(AppFetchLimit::Credit),
        ),
    ] {
        let run = start((0..10).map(|n| vec![n; 10]).collect()).await;
        settle().await;
        run.streams.control(run.connection, run.fetch_id, control);
        run.task.await.expect("ran");
        assert_eq!(run.sink.chunks(), INITIAL_CREDIT as usize, "{control:?}");
        match run.sink.last() {
            Some(InternalServiceResponse::AppFetchFailure(f)) => {
                assert_eq!(f.refusal, refusal, "{control:?}")
            }
            other => panic!("{control:?}: not refused: {other:?}"),
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_stream_waiting_for_credit_ends_at_the_time_budget() {
    let run = start((0..10).map(|n| vec![n; 10]).collect()).await;
    run.task.await.expect("ran");
    match run.sink.last() {
        Some(InternalServiceResponse::AppFetchFailure(f)) => {
            assert_eq!(f.refusal, AppFetchRefusal::Limit(AppFetchLimit::Time))
        }
        other => panic!("not refused: {other:?}"),
    }
}

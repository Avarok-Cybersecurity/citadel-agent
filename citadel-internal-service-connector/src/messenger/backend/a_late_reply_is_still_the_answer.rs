//! What happens to a waiter, and to its reply, when the reply is slow, the
//! caller gives up, or the connection goes.
//!
//! The end-to-end version (a whole messenger over a stalled fake agent) is in
//! `tests/a_slow_agent_is_not_a_failed_agent.rs`. These pin the slot's own
//! rules against the backend directly. Time is paused, so a minute-late reply
//! costs nothing and is ordered exactly against any timer the backend arms.

use super::*;
use crate::messenger::{OutboundFrame, StreamKey};
use citadel_internal_service_types::{InternalServicePayload, LocalDBGetKVSuccess};
use citadel_io::tokio;
use citadel_io::tokio::sync::mpsc::UnboundedReceiver;
use futures::FutureExt;
use intersession_layer_messaging::Payload;

const CID: u64 = 9;

type Outbound = UnboundedReceiver<(StreamKey, OutboundFrame<WrappedMessage>)>;

fn backend() -> (CitadelWorkspaceBackend, Outbound) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let backend = CitadelWorkspaceBackend::with_channel(
        CID,
        RequestChannel::new(
            CID,
            crate::messenger::BypasserTx {
                tx,
                stream_key: StreamKey::bypass_ism(),
            },
        ),
    );
    (backend, rx)
}

/// The agent's answer to the next `LocalDBGetKV` the backend sent.
async fn reply_to_next(outbound: &mut Outbound) -> InternalServiceResponse {
    let (_, frame) = outbound.recv().await.expect("a request was sent");
    let Payload::Message(message) = frame.payload else {
        panic!("the backend sends requests as messages")
    };
    let InternalServicePayload::Request(InternalServiceRequest::LocalDBGetKV {
        request_id,
        key,
        ..
    }) = message.contents
    else {
        panic!("expected a LocalDBGetKV")
    };
    InternalServiceResponse::LocalDBGetKVSuccess(LocalDBGetKVSuccess {
        cid: CID,
        peer_cid: None,
        key,
        value: vec![7],
        request_id: Some(request_id),
    })
}

#[tokio::test(start_paused = true)]
async fn a_minute_late_reply_completes_the_read() {
    let (backend, mut outbound) = backend();
    let agent = {
        let backend = backend.clone();
        async move {
            let reply = reply_to_next(&mut outbound).await;
            tokio::time::sleep(Duration::from_secs(60)).await;
            backend
                .inspect_received_payload(reply)
                .await
                .expect("inspect")
        }
    };
    let (value, unclaimed) = tokio::join!(backend.load_value("key"), agent);
    assert_eq!(value.ok(), Some(Some(vec![7])), "late is not failed");
    assert!(unclaimed.is_none(), "the reply was the backend's own");
    assert!(backend.channel.expected_requests.is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_reply_to_an_abandoned_read_is_still_consumed() {
    // A caller that stops waiting (its future dropped) must not turn the reply
    // into an unsolicited LocalDB answer for the application, and must not
    // leave a slot behind once that reply has come.
    let (backend, mut outbound) = backend();
    let mut read = Box::pin(backend.load_value("key"));
    assert!(
        (&mut read).now_or_never().is_none(),
        "sent, not yet answered"
    );
    drop(read);

    let reply = reply_to_next(&mut outbound).await;
    let unclaimed = backend
        .inspect_received_payload(reply)
        .await
        .expect("inspect");
    assert!(
        unclaimed.is_none(),
        "the backend's reply leaked to the application"
    );
    assert!(
        backend.channel.expected_requests.is_empty(),
        "the slot outlived its reply"
    );
}

#[tokio::test(start_paused = true)]
async fn losing_the_connection_fails_every_waiting_request_at_once() {
    let (backend, _outbound) = backend();
    let started = tokio::time::Instant::now();
    let abandon = {
        let backend = backend.clone();
        async move {
            // Both reads are registered and sent before this runs.
            tokio::task::yield_now().await;
            backend.abandon_all_requests();
        }
    };
    let (first, second, ()) =
        tokio::join!(backend.load_value("a"), backend.load_value("b"), abandon);
    for outcome in [first, second] {
        let err = format!("{:?}", outcome.expect_err("no answer can come"));
        assert!(err.contains("connection to the agent closed"), "{err}");
    }
    assert_eq!(
        started.elapsed(),
        Duration::ZERO,
        "failed by a deadline, not the loss"
    );
}

#[tokio::test(start_paused = true)]
async fn a_request_that_never_left_fails_its_waiter() {
    let (backend, mut outbound) = backend();
    let abandon = {
        let backend = backend.clone();
        async move {
            let (_, frame) = outbound.recv().await.expect("a request was sent");
            let Payload::Message(message) = frame.payload else {
                panic!("the backend sends requests as messages")
            };
            let InternalServicePayload::Request(request) = message.contents else {
                panic!("expected a request")
            };
            // What the messenger does when the sink refuses this request.
            backend.abandon_request(request.request_id().expect("LocalDB requests carry one"));
        }
    };
    let (outcome, ()) = tokio::join!(backend.load_value("key"), abandon);
    assert!(outcome.is_err());
    assert!(backend.channel.expected_requests.is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_request_id_already_in_flight_is_refused_not_displaced() {
    let (backend, _outbound) = backend();
    let id = Uuid::new_v4();
    let get = |key: &str| InternalServiceRequest::LocalDBGetKV {
        request_id: id,
        cid: CID,
        peer_cid: None,
        key: key.to_string(),
    };
    let mut first = Box::pin(backend.request(get("a"), id));
    assert!((&mut first).now_or_never().is_none());
    let second = backend.request(get("b"), id).await;
    assert!(
        second.is_err(),
        "the second must not take the first one's slot"
    );
    assert_eq!(
        backend.channel.expected_requests.len(),
        1,
        "the first is still waiting"
    );
}

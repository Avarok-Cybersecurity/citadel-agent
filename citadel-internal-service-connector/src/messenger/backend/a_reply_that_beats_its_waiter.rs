//! A reply that arrives before its waiter registers must still reach it.
//!
//! Every request here sent first and registered its reply slot second. The
//! agent is local and fast, so on a busy machine the reply can be inspected in
//! between: no slot matches, the reply is passed on as uncaught, and the
//! waiter times out five seconds later on an answer that already came. Seen on
//! a Windows CI runner as `multiplex` failing with "Timed out reading the
//! inbound_messages map", with the log showing the reply inspected before the
//! wait began.
//!
//! The responder below answers from its own OS thread the instant the request
//! is on the channel, which is the ordering the race needs. Without the fix a
//! round loses its reply well within these rounds; each loss costs a
//! five-second timeout and fails the round.

use super::*;
use crate::messenger::{OutboundFrame, StreamKey};
use citadel_internal_service_types::{InternalServicePayload, LocalDBGetKVSuccess};
use intersession_layer_messaging::Payload;
use std::sync::atomic::{AtomicBool, Ordering};

const ROUNDS: usize = 2000;
const CID: u64 = 7;

fn reply_to(frame: OutboundFrame<WrappedMessage>) -> InternalServiceResponse {
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
        value: vec![1],
        request_id: Some(request_id),
    })
}

#[citadel_io::tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_instant_reply_is_not_lost() {
    let (tx, mut rx) = citadel_io::tokio::sync::mpsc::unbounded_channel();
    let backend = CitadelWorkspaceBackend {
        cid: CID,
        expected_requests: Arc::new(DashMap::new()),
        bypass_ism_outbound_tx: Some(BypasserTx {
            tx,
            stream_key: StreamKey::bypass_ism(),
        }),
        outbound_gate: Arc::new(Mutex::new(())),
        inbound_gate: Arc::new(Mutex::new(())),
    };
    let stop = Arc::new(AtomicBool::new(false));
    let responder = {
        let backend = backend.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match rx.try_recv() {
                    Ok((_, frame)) => {
                        let unclaimed = futures::executor::block_on(
                            backend.inspect_received_payload(reply_to(frame)),
                        )
                        .expect("inspect");
                        assert!(unclaimed.is_none() || !stop.load(Ordering::Relaxed));
                    }
                    Err(_) => std::hint::spin_loop(),
                }
            }
        })
    };

    for round in 0..ROUNDS {
        let value = backend.load_value("key").await;
        assert_eq!(
            value.as_ref().ok(),
            Some(&Some(vec![1])),
            "round {round}: the reply arrived and the waiter still timed out: {value:?}"
        );
    }
    stop.store(true, Ordering::Relaxed);
    responder.join().expect("responder");
}
